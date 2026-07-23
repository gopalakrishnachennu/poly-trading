#![forbid(unsafe_code)]

//! Append-only, checksummed market-event journal.

use crc32fast::hash;
use event_schema::{EventEnvelope, SchemaError, MAX_MARKET_ID_BYTES, MAX_PAYLOAD_BYTES};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const FILE_MAGIC: &[u8; 8] = b"POLYJNL1";
const FILE_VERSION: u16 = 1;
const FILE_HEADER_LEN: usize = 16;
const FILE_HEADER_BYTES: u64 = 16;
const RECORD_HEADER_LEN: usize = 8;
const RECORD_HEADER_BYTES: u64 = 8;
const MAX_ENVELOPE_OVERHEAD: usize = 36;
const MAX_RECORD_BYTES: usize = MAX_ENVELOPE_OVERHEAD + MAX_MARKET_ID_BYTES + MAX_PAYLOAD_BYTES;

/// State of the bytes after the last valid record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalTail {
    Clean,
    Truncated { record_offset: u64 },
}

/// Result of a non-mutating journal scan.
#[derive(Debug, Eq, PartialEq)]
pub struct ScanReport {
    pub events: Vec<EventEnvelope>,
    pub valid_bytes: u64,
    pub tail: JournalTail,
}

/// Bounded-memory summary of a journal scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanSummary {
    pub event_count: u64,
    pub valid_bytes: u64,
    pub tail: JournalTail,
    pub last_sequence: Option<u64>,
}

/// Errors are conservative: corruption is never converted into a truncatable
/// crash tail.
#[derive(Debug)]
pub enum JournalError {
    Io(std::io::Error),
    InvalidFileHeader,
    UnsupportedFileVersion(u16),
    InvalidRecordLength { offset: u64, length: usize },
    ChecksumMismatch { offset: u64 },
    InvalidEnvelope { offset: u64, source: SchemaError },
    TruncatedTailRequiresRecovery { offset: u64 },
}

impl Display for JournalError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "journal I/O error: {error}"),
            Self::InvalidFileHeader => formatter.write_str("invalid journal file header"),
            Self::UnsupportedFileVersion(version) => {
                write!(formatter, "unsupported journal file version: {version}")
            }
            Self::InvalidRecordLength { offset, length } => {
                write!(
                    formatter,
                    "invalid record length {length} at offset {offset}"
                )
            }
            Self::ChecksumMismatch { offset } => {
                write!(formatter, "record checksum mismatch at offset {offset}")
            }
            Self::InvalidEnvelope { offset, source } => {
                write!(formatter, "invalid envelope at offset {offset}: {source}")
            }
            Self::TruncatedTailRequiresRecovery { offset } => {
                write!(formatter, "journal has a truncated tail at offset {offset}")
            }
        }
    }
}

impl Error for JournalError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidEnvelope { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for JournalError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// Writer for one journal segment.
#[derive(Debug)]
pub struct JournalWriter {
    file: File,
    position: u64,
    last_sequence: Option<u64>,
}

impl JournalWriter {
    /// Creates a brand-new segment and refuses an existing target.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] when creation or initial synchronization
    /// fails.
    pub fn create_new(path: impl AsRef<Path>) -> Result<Self, JournalError> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)?;
        write_file_header(&mut file)?;
        file.sync_data()?;
        Ok(Self {
            file,
            position: FILE_HEADER_BYTES,
            last_sequence: None,
        })
    }

    /// Opens a clean segment or initializes an empty one.
    ///
    /// A truncated segment must be explicitly recovered first. This prevents an
    /// ordinary restart from silently discarding bytes.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError`] for I/O failures, invalid headers, corruption,
    /// unsupported versions, or an unrecovered truncated tail.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, JournalError> {
        let path = path.as_ref();
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;

        let length = file.metadata()?.len();
        let last_sequence = if length == 0 {
            write_file_header(&mut file)?;
            file.sync_data()?;
            None
        } else {
            let report = scan_summary(path)?;
            if let JournalTail::Truncated { record_offset } = report.tail {
                return Err(JournalError::TruncatedTailRequiresRecovery {
                    offset: record_offset,
                });
            }
            report.last_sequence
        };

        let position = file.seek(SeekFrom::End(0))?;
        Ok(Self {
            file,
            position,
            last_sequence,
        })
    }

    /// Returns the greatest source sequence already present in this segment.
    ///
    /// This is a restart hint for a single recorder stream. Callers remain
    /// responsible for source-specific sequence semantics.
    #[must_use]
    pub const fn last_sequence(&self) -> Option<u64> {
        self.last_sequence
    }

    #[must_use]
    pub const fn position(&self) -> u64 {
        self.position
    }

    /// Appends and flushes one complete record to the operating system.
    ///
    /// Call [`Self::sync`] at a durability boundary to request device sync.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError`] when encoding, validation, or I/O fails.
    pub fn append(&mut self, event: &EventEnvelope) -> Result<u64, JournalError> {
        let encoded = event
            .encode()
            .map_err(|source| JournalError::InvalidEnvelope {
                offset: self.position,
                source,
            })?;
        if encoded.is_empty() || encoded.len() > MAX_RECORD_BYTES {
            return Err(JournalError::InvalidRecordLength {
                offset: self.position,
                length: encoded.len(),
            });
        }
        let length =
            u32::try_from(encoded.len()).map_err(|_| JournalError::InvalidRecordLength {
                offset: self.position,
                length: encoded.len(),
            })?;
        let record_offset = self.position;

        self.file.write_all(&length.to_le_bytes())?;
        self.file.write_all(&hash(&encoded).to_le_bytes())?;
        self.file.write_all(&encoded)?;
        self.file.flush()?;
        self.position = self
            .position
            .checked_add(RECORD_HEADER_BYTES)
            .and_then(|value| value.checked_add(u64::from(length)))
            .ok_or(JournalError::InvalidRecordLength {
                offset: record_offset,
                length: encoded.len(),
            })?;
        self.last_sequence = Some(
            self.last_sequence
                .map_or(event.sequence, |current| current.max(event.sequence)),
        );
        Ok(record_offset)
    }

    /// Requests that appended data reach the storage device.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] if the device synchronization fails.
    pub fn sync(&self) -> Result<(), JournalError> {
        self.file.sync_data().map_err(JournalError::Io)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SegmentConfig {
    pub max_segment_bytes: u64,
    pub max_segment_records: u64,
}

impl Default for SegmentConfig {
    fn default() -> Self {
        Self {
            max_segment_bytes: 256 * 1024 * 1024,
            max_segment_records: 1_000_000,
        }
    }
}

#[derive(Debug)]
pub enum SegmentError {
    Journal(JournalError),
    Io(std::io::Error),
    InvalidConfig,
    InvalidDirectory,
    UnexpectedEntry(PathBuf),
    SegmentGap { expected: u64, actual: u64 },
    IncompleteSegment { index: u64 },
    SequenceRegression { expected: u64, actual: u64 },
    SequenceGap { expected: u64, actual: u64 },
    SequenceExhausted,
    SegmentIndexOverflow,
    RecordLengthOverflow,
}

impl Display for SegmentError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Journal(error) => write!(formatter, "segment journal error: {error}"),
            Self::Io(error) => write!(formatter, "segment I/O error: {error}"),
            Self::InvalidConfig => formatter.write_str("invalid segment configuration"),
            Self::InvalidDirectory => formatter.write_str("invalid or symbolic segment directory"),
            Self::UnexpectedEntry(path) => {
                write!(
                    formatter,
                    "unexpected segment directory entry: {}",
                    path.display()
                )
            }
            Self::SegmentGap { expected, actual } => {
                write!(
                    formatter,
                    "segment index gap: expected {expected}, found {actual}"
                )
            }
            Self::IncompleteSegment { index } => {
                write!(formatter, "segment {index} has an incomplete tail")
            }
            Self::SequenceRegression { expected, actual } => write!(
                formatter,
                "cross-segment sequence regression: expected {expected}, found {actual}"
            ),
            Self::SequenceGap { expected, actual } => write!(
                formatter,
                "cross-segment sequence gap: expected {expected}, found {actual}"
            ),
            Self::SequenceExhausted => formatter.write_str("segment sequence exhausted"),
            Self::SegmentIndexOverflow => formatter.write_str("segment index overflow"),
            Self::RecordLengthOverflow => formatter.write_str("segment record length overflow"),
        }
    }
}

impl Error for SegmentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Journal(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<JournalError> for SegmentError {
    fn from(value: JournalError) -> Self {
        Self::Journal(value)
    }
}

impl From<std::io::Error> for SegmentError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug)]
pub enum JournalBackendError {
    Single(JournalError),
    Segmented(SegmentError),
}

impl Display for JournalBackendError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Single(error) => Display::fmt(error, formatter),
            Self::Segmented(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for JournalBackendError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Single(error) => Some(error),
            Self::Segmented(error) => Some(error),
        }
    }
}

/// Minimal append/sync boundary shared by single and segmented journals.
pub trait EventJournal {
    /// Appends one validated envelope.
    ///
    /// # Errors
    ///
    /// Returns [`JournalBackendError`] for encoding, ordering, rotation, or I/O
    /// failures.
    fn append_event(&mut self, event: &EventEnvelope) -> Result<u64, JournalBackendError>;

    /// Synchronizes the active durable boundary.
    ///
    /// # Errors
    ///
    /// Returns [`JournalBackendError`] for storage synchronization failure.
    fn sync_events(&self) -> Result<(), JournalBackendError>;

    fn last_event_sequence(&self) -> Option<u64>;
}

impl EventJournal for JournalWriter {
    fn append_event(&mut self, event: &EventEnvelope) -> Result<u64, JournalBackendError> {
        self.append(event).map_err(JournalBackendError::Single)
    }

    fn sync_events(&self) -> Result<(), JournalBackendError> {
        self.sync().map_err(JournalBackendError::Single)
    }

    fn last_event_sequence(&self) -> Option<u64> {
        self.last_sequence()
    }
}

#[derive(Debug)]
pub struct SegmentedJournalWriter {
    directory: PathBuf,
    config: SegmentConfig,
    writer: JournalWriter,
    segment_index: u64,
    segment_records: u64,
    last_sequence: Option<u64>,
}

impl SegmentedJournalWriter {
    /// Opens or creates a validated append-only segment directory.
    ///
    /// # Errors
    ///
    /// Returns [`SegmentError`] for invalid configuration, directory layout,
    /// segment integrity, or cross-segment ordering.
    pub fn open(directory: impl AsRef<Path>, config: SegmentConfig) -> Result<Self, SegmentError> {
        validate_segment_config(config)?;
        let directory = directory.as_ref().to_path_buf();
        ensure_directory(&directory)?;
        let mut segments = segment_paths(&directory)?;
        if segments.is_empty() {
            let path = directory.join(segment_name(0));
            let writer = JournalWriter::create_new(path)?;
            return Ok(Self {
                directory,
                config,
                writer,
                segment_index: 0,
                segment_records: 0,
                last_sequence: None,
            });
        }
        segments.sort_by_key(|(index, _)| *index);
        validate_segment_indices(&segments)?;
        let mut stream = SegmentedJournalReader::from_segments(segments.clone())?;
        let mut last_sequence = None;
        while let Some(event) = stream.next_event()? {
            last_sequence = Some(event.sequence);
        }
        let (segment_index, path) = segments
            .last()
            .cloned()
            .ok_or(SegmentError::InvalidDirectory)?;
        let summary = scan_summary(&path)?;
        if summary.tail != JournalTail::Clean {
            return Err(SegmentError::IncompleteSegment {
                index: segment_index,
            });
        }
        let writer = JournalWriter::open(path)?;
        Ok(Self {
            directory,
            config,
            writer,
            segment_index,
            segment_records: summary.event_count,
            last_sequence,
        })
    }

    /// Appends one contiguous event, rotating before configured limits.
    ///
    /// # Errors
    ///
    /// Returns [`SegmentError`] for sequence, encoding, rotation, or I/O
    /// failures.
    pub fn append(&mut self, event: &EventEnvelope) -> Result<u64, SegmentError> {
        validate_next_sequence(self.last_sequence, event.sequence)?;
        let encoded_length = u64::try_from(
            event
                .encode()
                .map_err(|source| JournalError::InvalidEnvelope {
                    offset: self.writer.position(),
                    source,
                })?
                .len(),
        )
        .map_err(|_| SegmentError::RecordLengthOverflow)?;
        let record_bytes = RECORD_HEADER_BYTES
            .checked_add(encoded_length)
            .ok_or(SegmentError::RecordLengthOverflow)?;
        let projected = self
            .writer
            .position()
            .checked_add(record_bytes)
            .ok_or(SegmentError::RecordLengthOverflow)?;
        if self.segment_records > 0
            && (self.segment_records >= self.config.max_segment_records
                || projected > self.config.max_segment_bytes)
        {
            self.rotate()?;
        }
        let offset = self.writer.append(event)?;
        self.segment_records = self
            .segment_records
            .checked_add(1)
            .ok_or(SegmentError::RecordLengthOverflow)?;
        self.last_sequence = Some(event.sequence);
        Ok(offset)
    }

    /// Synchronizes the active segment to its storage device.
    ///
    /// # Errors
    ///
    /// Returns [`SegmentError`] when device synchronization fails.
    pub fn sync(&self) -> Result<(), SegmentError> {
        self.writer.sync().map_err(SegmentError::Journal)
    }

    #[must_use]
    pub const fn last_sequence(&self) -> Option<u64> {
        self.last_sequence
    }

    #[must_use]
    pub const fn segment_index(&self) -> u64 {
        self.segment_index
    }

    fn rotate(&mut self) -> Result<(), SegmentError> {
        self.writer.sync()?;
        let next = self
            .segment_index
            .checked_add(1)
            .ok_or(SegmentError::SegmentIndexOverflow)?;
        let writer = JournalWriter::create_new(self.directory.join(segment_name(next)))?;
        self.writer = writer;
        self.segment_index = next;
        self.segment_records = 0;
        Ok(())
    }
}

impl EventJournal for SegmentedJournalWriter {
    fn append_event(&mut self, event: &EventEnvelope) -> Result<u64, JournalBackendError> {
        self.append(event).map_err(JournalBackendError::Segmented)
    }

    fn sync_events(&self) -> Result<(), JournalBackendError> {
        self.sync().map_err(JournalBackendError::Segmented)
    }

    fn last_event_sequence(&self) -> Option<u64> {
        self.last_sequence()
    }
}

#[derive(Debug)]
pub struct SegmentedJournalReader {
    segments: Vec<(u64, PathBuf)>,
    current_index: usize,
    current: JournalReader,
    last_sequence: Option<u64>,
}

impl SegmentedJournalReader {
    /// Opens a non-empty, contiguous, validated segment directory.
    ///
    /// # Errors
    ///
    /// Returns [`SegmentError`] for directory, index, symlink, or journal
    /// failures.
    pub fn open(directory: impl AsRef<Path>) -> Result<Self, SegmentError> {
        let directory = directory.as_ref();
        validate_existing_directory(directory)?;
        let mut segments = segment_paths(directory)?;
        if segments.is_empty() {
            return Err(SegmentError::InvalidDirectory);
        }
        segments.sort_by_key(|(index, _)| *index);
        validate_segment_indices(&segments)?;
        Self::from_segments(segments)
    }

    fn from_segments(segments: Vec<(u64, PathBuf)>) -> Result<Self, SegmentError> {
        let first = segments.first().ok_or(SegmentError::InvalidDirectory)?;
        let current = JournalReader::open(&first.1)?;
        Ok(Self {
            segments,
            current_index: 0,
            current,
            last_sequence: None,
        })
    }

    /// Returns the next contiguous event across segment boundaries.
    ///
    /// # Errors
    ///
    /// Returns [`SegmentError`] for segment corruption, incomplete tails, or
    /// sequence discontinuity.
    pub fn next_event(&mut self) -> Result<Option<EventEnvelope>, SegmentError> {
        loop {
            if let Some(event) = self.current.next_event()? {
                validate_next_sequence(self.last_sequence, event.sequence)?;
                self.last_sequence = Some(event.sequence);
                return Ok(Some(event));
            }
            if self.current.tail() != Some(JournalTail::Clean) {
                return Err(SegmentError::IncompleteSegment {
                    index: self.segments[self.current_index].0,
                });
            }
            let next = self.current_index + 1;
            if next == self.segments.len() {
                return Ok(None);
            }
            self.current = JournalReader::open(&self.segments[next].1)?;
            self.current_index = next;
        }
    }

    #[must_use]
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }
}

fn validate_segment_config(config: SegmentConfig) -> Result<(), SegmentError> {
    if config.max_segment_bytes <= FILE_HEADER_BYTES + RECORD_HEADER_BYTES
        || config.max_segment_records == 0
    {
        Err(SegmentError::InvalidConfig)
    } else {
        Ok(())
    }
}

fn ensure_directory(directory: &Path) -> Result<(), SegmentError> {
    if directory.exists() {
        validate_existing_directory(directory)
    } else {
        fs::create_dir_all(directory)?;
        validate_existing_directory(directory)
    }
}

fn validate_existing_directory(directory: &Path) -> Result<(), SegmentError> {
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        Err(SegmentError::InvalidDirectory)
    } else {
        Ok(())
    }
}

fn segment_paths(directory: &Path) -> Result<Vec<(u64, PathBuf)>, SegmentError> {
    let mut result = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(SegmentError::UnexpectedEntry(path));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| SegmentError::UnexpectedEntry(path.clone()))?;
        let index =
            parse_segment_name(&name).ok_or_else(|| SegmentError::UnexpectedEntry(path.clone()))?;
        result.push((index, path));
    }
    Ok(result)
}

fn validate_segment_indices(segments: &[(u64, PathBuf)]) -> Result<(), SegmentError> {
    for (expected, (actual, _)) in segments.iter().enumerate() {
        let expected = u64::try_from(expected).map_err(|_| SegmentError::SegmentIndexOverflow)?;
        if *actual != expected {
            return Err(SegmentError::SegmentGap {
                expected,
                actual: *actual,
            });
        }
    }
    Ok(())
}

fn validate_next_sequence(last: Option<u64>, actual: u64) -> Result<(), SegmentError> {
    let Some(last) = last else {
        return Ok(());
    };
    let expected = last.checked_add(1).ok_or(SegmentError::SequenceExhausted)?;
    match actual.cmp(&expected) {
        std::cmp::Ordering::Less => Err(SegmentError::SequenceRegression { expected, actual }),
        std::cmp::Ordering::Greater => Err(SegmentError::SequenceGap { expected, actual }),
        std::cmp::Ordering::Equal => Ok(()),
    }
}

fn segment_name(index: u64) -> String {
    format!("segment-{index:020}.journal")
}

fn parse_segment_name(name: &str) -> Option<u64> {
    let digits = name.strip_prefix("segment-")?.strip_suffix(".journal")?;
    if digits.len() != 20 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let index = digits.parse().ok()?;
    (segment_name(index) == name).then_some(index)
}

/// One-record-at-a-time reader for a checksummed journal segment.
#[derive(Debug)]
pub struct JournalReader {
    reader: BufReader<File>,
    position: u64,
    finished: bool,
    tail: Option<JournalTail>,
}

impl JournalReader {
    /// Opens a segment and validates its file header without collecting events.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError`] for I/O, magic, or version failures. A partial
    /// file header is represented as a truncated tail.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, JournalError> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut header = [0_u8; FILE_HEADER_LEN];
        let header_read = fill_until_eof(&mut reader, &mut header)?;
        if header_read != header.len() {
            return Ok(Self {
                reader,
                position: 0,
                finished: true,
                tail: Some(JournalTail::Truncated { record_offset: 0 }),
            });
        }
        validate_file_header(&header)?;
        Ok(Self {
            reader,
            position: FILE_HEADER_BYTES,
            finished: false,
            tail: None,
        })
    }

    /// Decodes the next complete event while retaining at most one record body.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError`] for hard length, checksum, schema, or I/O
    /// failures. An incomplete tail ends iteration and is exposed by
    /// [`Self::tail`].
    pub fn next_event(&mut self) -> Result<Option<EventEnvelope>, JournalError> {
        if self.finished {
            return Ok(None);
        }
        let record_offset = self.position;
        let mut record_header = [0_u8; RECORD_HEADER_LEN];
        let header_read = fill_until_eof(&mut self.reader, &mut record_header)?;
        if header_read == 0 {
            self.finish(JournalTail::Clean);
            return Ok(None);
        }
        if header_read != record_header.len() {
            self.finish(JournalTail::Truncated { record_offset });
            return Ok(None);
        }

        let encoded_length = u32::from_le_bytes([
            record_header[0],
            record_header[1],
            record_header[2],
            record_header[3],
        ]);
        let length =
            usize::try_from(encoded_length).map_err(|_| JournalError::InvalidRecordLength {
                offset: record_offset,
                length: usize::MAX,
            })?;
        if length == 0 || length > MAX_RECORD_BYTES {
            return Err(JournalError::InvalidRecordLength {
                offset: record_offset,
                length,
            });
        }
        let expected_checksum = u32::from_le_bytes([
            record_header[4],
            record_header[5],
            record_header[6],
            record_header[7],
        ]);
        let mut encoded = vec![0_u8; length];
        let body_read = fill_until_eof(&mut self.reader, &mut encoded)?;
        if body_read != length {
            self.finish(JournalTail::Truncated { record_offset });
            return Ok(None);
        }
        if hash(&encoded) != expected_checksum {
            return Err(JournalError::ChecksumMismatch {
                offset: record_offset,
            });
        }
        let event =
            EventEnvelope::decode(&encoded).map_err(|source| JournalError::InvalidEnvelope {
                offset: record_offset,
                source,
            })?;
        self.position = self
            .position
            .checked_add(RECORD_HEADER_BYTES)
            .and_then(|value| value.checked_add(u64::from(encoded_length)))
            .ok_or(JournalError::InvalidRecordLength {
                offset: record_offset,
                length,
            })?;
        Ok(Some(event))
    }

    #[must_use]
    pub const fn valid_bytes(&self) -> u64 {
        self.position
    }

    #[must_use]
    pub const fn tail(&self) -> Option<JournalTail> {
        self.tail
    }

    fn finish(&mut self, tail: JournalTail) {
        self.finished = true;
        self.tail = Some(tail);
    }
}

/// Scans without changing the journal.
///
/// # Errors
///
/// Returns [`JournalError`] for I/O errors or hard structural/checksum
/// corruption. An incomplete crash tail is returned in [`ScanReport`].
pub fn scan_path(path: impl AsRef<Path>) -> Result<ScanReport, JournalError> {
    let mut reader = JournalReader::open(path)?;
    let mut events = Vec::new();
    while let Some(event) = reader.next_event()? {
        events.push(event);
    }
    Ok(ScanReport {
        events,
        valid_bytes: reader.valid_bytes(),
        tail: reader.tail().unwrap_or(JournalTail::Clean),
    })
}

/// Scans a segment without retaining decoded historical events.
///
/// # Errors
///
/// Returns [`JournalError`] for hard corruption or I/O failures.
pub fn scan_summary(path: impl AsRef<Path>) -> Result<ScanSummary, JournalError> {
    let mut reader = JournalReader::open(path)?;
    let mut event_count = 0_u64;
    let mut last_sequence = None;
    while let Some(event) = reader.next_event()? {
        event_count = event_count
            .checked_add(1)
            .ok_or(JournalError::InvalidRecordLength {
                offset: reader.valid_bytes(),
                length: 0,
            })?;
        last_sequence =
            Some(last_sequence.map_or(event.sequence, |current: u64| current.max(event.sequence)));
    }
    Ok(ScanSummary {
        event_count,
        valid_bytes: reader.valid_bytes(),
        tail: reader.tail().unwrap_or(JournalTail::Clean),
        last_sequence,
    })
}

/// Removes only an incomplete crash tail and returns the remaining valid bytes.
/// Corruption is returned as an error and the file remains untouched.
///
/// # Errors
///
/// Returns [`JournalError`] for I/O errors or hard corruption, neither of which
/// causes truncation.
pub fn recover_truncated_tail(path: impl AsRef<Path>) -> Result<u64, JournalError> {
    let path = path.as_ref();
    let report = scan_summary(path)?;
    if matches!(report.tail, JournalTail::Truncated { .. }) {
        let file = OpenOptions::new().write(true).open(path)?;
        file.set_len(report.valid_bytes)?;
        file.sync_all()?;
    }
    Ok(report.valid_bytes)
}

fn write_file_header(file: &mut File) -> Result<(), JournalError> {
    let mut header = [0_u8; FILE_HEADER_LEN];
    header[0..8].copy_from_slice(FILE_MAGIC);
    header[8..10].copy_from_slice(&FILE_VERSION.to_le_bytes());
    file.write_all(&header)?;
    file.flush()?;
    Ok(())
}

fn validate_file_header(header: &[u8; FILE_HEADER_LEN]) -> Result<(), JournalError> {
    if &header[0..8] != FILE_MAGIC || header[10..].iter().any(|byte| *byte != 0) {
        return Err(JournalError::InvalidFileHeader);
    }
    let version = u16::from_le_bytes([header[8], header[9]]);
    if version != FILE_VERSION {
        return Err(JournalError::UnsupportedFileVersion(version));
    }
    Ok(())
}

fn fill_until_eof(reader: &mut impl Read, buffer: &mut [u8]) -> Result<usize, std::io::Error> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..])? {
            0 => break,
            count => filled += count,
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_schema::EventSource;
    use std::fs;
    use tempfile::tempdir;

    fn event(sequence: u64) -> EventEnvelope {
        EventEnvelope::new(
            EventSource::Market,
            sequence,
            1_000,
            1_100,
            "btc-hourly".to_owned(),
            vec![u8::try_from(sequence).unwrap_or(u8::MAX)],
        )
        .expect("valid event")
    }

    #[test]
    fn appends_and_scans_in_order() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("events.journal");
        let mut writer = JournalWriter::open(&path).expect("open");
        writer.append(&event(1)).expect("append 1");
        writer.append(&event(2)).expect("append 2");
        writer.sync().expect("sync");
        drop(writer);

        let report = scan_path(&path).expect("scan");
        assert_eq!(report.tail, JournalTail::Clean);
        assert_eq!(report.events, vec![event(1), event(2)]);
        assert_eq!(
            report.valid_bytes,
            fs::metadata(path).expect("metadata").len()
        );
    }

    #[test]
    fn preserves_sequence_hint_across_reopen() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("events.journal");
        let mut writer = JournalWriter::open(&path).expect("open");
        assert_eq!(writer.last_sequence(), None);
        writer.append(&event(4)).expect("append 4");
        writer.append(&event(2)).expect("append 2");
        assert_eq!(writer.last_sequence(), Some(4));
        writer.sync().expect("sync");
        drop(writer);

        let reopened = JournalWriter::open(path).expect("reopen");
        assert_eq!(reopened.last_sequence(), Some(4));
    }

    #[test]
    fn recovers_partial_record_body() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("events.journal");
        let mut writer = JournalWriter::open(&path).expect("open");
        writer.append(&event(1)).expect("append 1");
        writer.append(&event(2)).expect("append 2");
        writer.sync().expect("sync");
        drop(writer);

        let original = fs::metadata(&path).expect("metadata").len();
        OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open to truncate")
            .set_len(original - 1)
            .expect("truncate");

        let report = scan_path(&path).expect("scan truncated");
        assert!(matches!(report.tail, JournalTail::Truncated { .. }));
        assert_eq!(report.events, vec![event(1)]);
        let valid_bytes = report.valid_bytes;

        assert_eq!(recover_truncated_tail(&path).expect("recover"), valid_bytes);
        let recovered = scan_path(&path).expect("scan recovered");
        assert_eq!(recovered.tail, JournalTail::Clean);
        assert_eq!(recovered.events, vec![event(1)]);
    }

    #[test]
    fn recovers_partial_record_header() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("events.journal");
        let mut writer = JournalWriter::open(&path).expect("open");
        writer.append(&event(1)).expect("append");
        writer.sync().expect("sync");
        drop(writer);

        let clean_size = fs::metadata(&path).expect("metadata").len();
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open append");
        file.write_all(&[1, 2, 3]).expect("partial header");
        file.flush().expect("flush");
        drop(file);

        let report = scan_path(&path).expect("scan");
        assert_eq!(report.valid_bytes, clean_size);
        assert!(matches!(report.tail, JournalTail::Truncated { .. }));
        recover_truncated_tail(&path).expect("recover");
        assert_eq!(fs::metadata(path).expect("metadata").len(), clean_size);
    }

    #[test]
    fn checksum_corruption_halts_and_is_not_truncated() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("events.journal");
        let mut writer = JournalWriter::open(&path).expect("open");
        writer.append(&event(1)).expect("append");
        writer.sync().expect("sync");
        drop(writer);

        let original_size = fs::metadata(&path).expect("metadata").len();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open corrupt");
        file.seek(SeekFrom::Start(FILE_HEADER_BYTES + RECORD_HEADER_BYTES))
            .expect("seek");
        let mut byte = [0_u8; 1];
        file.read_exact(&mut byte).expect("read byte");
        file.seek(SeekFrom::Current(-1)).expect("seek back");
        byte[0] ^= 0xff;
        file.write_all(&byte).expect("write corrupt byte");
        file.flush().expect("flush");
        drop(file);

        assert!(matches!(
            scan_path(&path),
            Err(JournalError::ChecksumMismatch { .. })
        ));
        assert!(matches!(
            recover_truncated_tail(&path),
            Err(JournalError::ChecksumMismatch { .. })
        ));
        assert_eq!(fs::metadata(path).expect("metadata").len(), original_size);
    }

    #[test]
    fn writer_requires_explicit_recovery() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("events.journal");
        let writer = JournalWriter::open(&path).expect("open");
        drop(writer);
        OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open append")
            .write_all(&[9])
            .expect("write tail");

        assert!(matches!(
            JournalWriter::open(path),
            Err(JournalError::TruncatedTailRequiresRecovery { .. })
        ));
    }

    #[test]
    fn streaming_and_collecting_scans_agree() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("stream.journal");
        let mut writer = JournalWriter::open(&path).expect("open");
        for sequence in 0..1_000 {
            writer.append(&event(sequence)).expect("append");
        }
        writer.sync().expect("sync");
        drop(writer);

        let collected = scan_path(&path).expect("collect");
        let summary = scan_summary(&path).expect("summary");
        assert_eq!(summary.event_count, 1_000);
        assert_eq!(summary.valid_bytes, collected.valid_bytes);
        assert_eq!(summary.tail, collected.tail);
        assert_eq!(summary.last_sequence, Some(999));

        let mut reader = JournalReader::open(path).expect("reader");
        let mut streamed = Vec::new();
        while let Some(next) = reader.next_event().expect("next") {
            streamed.push(next);
        }
        assert_eq!(streamed, collected.events);
        assert_eq!(reader.tail(), Some(JournalTail::Clean));
    }

    #[test]
    fn segmented_writer_rotates_and_restarts_contiguously() {
        let directory = tempdir().expect("tempdir");
        let config = SegmentConfig {
            max_segment_bytes: u64::MAX,
            max_segment_records: 2,
        };
        let mut writer = SegmentedJournalWriter::open(directory.path(), config).expect("writer");
        for sequence in 0..5 {
            writer.append(&event(sequence)).expect("append");
        }
        writer.sync().expect("sync");
        assert_eq!(writer.segment_index(), 2);
        drop(writer);

        let mut reopened =
            SegmentedJournalWriter::open(directory.path(), config).expect("reopen writer");
        assert_eq!(reopened.last_sequence(), Some(4));
        reopened.append(&event(5)).expect("restart append");
        reopened.sync().expect("sync");
        drop(reopened);

        let mut reader = SegmentedJournalReader::open(directory.path()).expect("reader");
        assert_eq!(reader.segment_count(), 3);
        let mut sequences = Vec::new();
        while let Some(next) = reader.next_event().expect("next") {
            sequences.push(next.sequence);
        }
        assert_eq!(sequences, (0..6).collect::<Vec<_>>());
    }

    #[test]
    fn byte_rotation_allows_one_oversized_valid_record() {
        let directory = tempdir().expect("tempdir");
        let config = SegmentConfig {
            max_segment_bytes: FILE_HEADER_BYTES + RECORD_HEADER_BYTES + 1,
            max_segment_records: 100,
        };
        let mut writer = SegmentedJournalWriter::open(directory.path(), config).expect("writer");
        writer.append(&event(0)).expect("first oversized record");
        writer.append(&event(1)).expect("rotate second record");
        writer.sync().expect("sync");
        assert_eq!(writer.segment_index(), 1);
    }

    #[test]
    fn segmented_reader_rejects_index_and_sequence_gaps() {
        let index_gap = tempdir().expect("tempdir");
        let mut zero = JournalWriter::create_new(
            index_gap
                .path()
                .join("segment-00000000000000000000.journal"),
        )
        .expect("zero");
        zero.append(&event(0)).expect("append zero");
        zero.sync().expect("sync zero");
        drop(zero);
        JournalWriter::create_new(
            index_gap
                .path()
                .join("segment-00000000000000000002.journal"),
        )
        .expect("two");
        assert!(matches!(
            SegmentedJournalReader::open(index_gap.path()),
            Err(SegmentError::SegmentGap {
                expected: 1,
                actual: 2
            })
        ));

        let sequence_gap = tempdir().expect("tempdir");
        let mut first = JournalWriter::create_new(
            sequence_gap
                .path()
                .join("segment-00000000000000000000.journal"),
        )
        .expect("first");
        first.append(&event(0)).expect("append zero");
        first.sync().expect("sync first");
        drop(first);
        let mut second = JournalWriter::create_new(
            sequence_gap
                .path()
                .join("segment-00000000000000000001.journal"),
        )
        .expect("second");
        second.append(&event(2)).expect("append two");
        second.sync().expect("sync second");
        drop(second);
        let mut reader = SegmentedJournalReader::open(sequence_gap.path()).expect("reader");
        assert_eq!(
            reader.next_event().expect("zero").expect("event").sequence,
            0
        );
        assert!(matches!(
            reader.next_event(),
            Err(SegmentError::SequenceGap {
                expected: 1,
                actual: 2
            })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn segmented_reader_rejects_symlink_entries() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().expect("tempdir");
        let outside = directory.path().join("outside");
        fs::write(&outside, b"not a segment").expect("outside");
        symlink(
            &outside,
            directory
                .path()
                .join("segment-00000000000000000000.journal"),
        )
        .expect("symlink");
        assert!(matches!(
            SegmentedJournalReader::open(directory.path()),
            Err(SegmentError::UnexpectedEntry(_))
        ));
    }
}

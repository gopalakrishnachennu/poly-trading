# Running the capture 24/7 (free)

You do **not** need to host the whole platform. Only the **read-only capture**
must run continuously so backtest data accumulates over weeks. It is tiny
(<150 MB RAM, ~2–3 MB/day disk), credentialless, and binds to loopback — cheap
and safe to run anywhere. Analysis, the backtest, and the terminal UI run
on-demand later.

What keeps running: `scripts/run-continuous-capture.sh` — the
`terminal-projection` gateway plus the compact snapshot recorder
(`scripts/capture_snapshots.py`), writing to `var/research-capture/`.

## Recommended free host: Oracle Cloud "Always Free"

Oracle's Always Free tier includes an Ampere Arm VM (up to 4 cores / 24 GB RAM)
and 200 GB storage, permanently — enough to build and run this natively. Google
Cloud's always-free `e2-micro` (1 GB) or a Raspberry Pi at home work too with
the same `systemd` setup (on the tiny `e2-micro`, prefer the Docker image or a
prebuilt binary over compiling on the box).

### 1. Create the VM

- Oracle Cloud → Compute → Instances → Create. Pick an **Always Free eligible**
  Ampere (Arm) shape, Ubuntu 22.04+. Allow only SSH inbound (the gateway stays
  on loopback — no public port needed).

### 2. Install prerequisites (on the VM)

```bash
sudo apt-get update
sudo apt-get install -y git python3 curl build-essential pkg-config libssl-dev
# Rust (pinned toolchain is read from rust-toolchain.toml)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"
```

### 3. Get the code

```bash
sudo mkdir -p /opt/poly-trading && sudo chown "$USER" /opt/poly-trading
git clone https://github.com/gopalakrishnachennu/poly-trading.git /opt/poly-trading
cd /opt/poly-trading
# Warm the build once (first service start would otherwise take a while):
cargo build --release -p terminal-projection
```

### 4. Run it as a boot-persistent service

```bash
# Create a dedicated user (optional but recommended) OR edit User= in the unit.
sudo useradd --system --home /opt/poly-trading poly 2>/dev/null || true
sudo chown -R poly /opt/poly-trading

sudo cp deploy/systemd/poly-capture.service /etc/systemd/system/
# Edit User=/WorkingDirectory= in the unit if your paths differ.
sudo systemctl daemon-reload
sudo systemctl enable --now poly-capture
```

Check it:

```bash
systemctl status poly-capture
journalctl -u poly-capture -f
cat /opt/poly-trading/var/capture-status.txt
```

It now runs across reboots and restarts on crash. Your laptop can be off.

### 5. Watch progress and pull data when ready

From the VM (or your laptop after `git pull` of the data, or `scp`):

```bash
python3 scripts/capture_progress.py           # resolved markets accumulated
python3 scripts/backtest_fair_value.py --glob 'var/research-capture/*.jsonl' \
    --walk-forward --sweep                     # once ~200–500 markets exist
```

`var/` is Git-ignored, so the data stays on the VM. To get it onto your laptop,
`scp -r user@vm:/opt/poly-trading/var/research-capture ./var/`, or push it to
cheap object storage on a timer.

## Zero-VM alternative: GitHub Actions cron (no signup, fully free)

If you would rather not run a VM, a scheduled workflow can capture in bursts and
commit the results to a data branch. Trade-offs: it samples **sparsely** (a
short burst every ~20–30 min, not continuous), cannot run a true daemon, and
commits data into the repo. It is enough to collect a strike and some in-hour
prices per market, but continuous capture on a VM is strictly better for the
backtest. Ask and I can add `.github/workflows/capture.yml` for this.

## Notes

- **Safety:** the gateway is read-only and credentialless; there is nothing
  secret to leak on the host. Keep the gateway on `127.0.0.1` (default) and do
  not expose port 8088 publicly.
- **Disk:** the compact recorder is ~2–3 MB/day. Do **not** run the raw
  tick-capture journals on a small free disk (they grow ~1 GB per 5 h).
- **Oracle idle reclaim:** Always-Free compute can be reclaimed if idle; this
  workload keeps light continuous activity, which helps. Keep the instance in a
  free-eligible shape to avoid charges.

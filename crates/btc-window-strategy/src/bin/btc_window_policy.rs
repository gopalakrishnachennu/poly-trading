//! CLI for Spec 096 window policy: schema, defaults, validate, activate.

use btc_window_strategy::{
    late_continuation_parameter_schema, list_plugins, LateContinuationParams, PolicyStore,
    SetupPolicyBag, WindowPolicyDocument,
};
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

fn policy_root() -> PathBuf {
    env::var("POLY_STRATEGY_CONTROL_DIR")
        .map_or_else(|_| PathBuf::from("var/strategy-control"), PathBuf::from)
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "help".into());
    let root = policy_root();
    let store = PolicyStore::new(&root);

    match cmd.as_str() {
        "help" | "-h" | "--help" => {
            eprintln!(
                "usage: btc-window-policy <schema|plugins|defaults|digest|save-defaults|activate VERSION|disable IDS>"
            );
            ExitCode::SUCCESS
        }
        "schema" => {
            println!("{}", late_continuation_parameter_schema());
            ExitCode::SUCCESS
        }
        "plugins" => {
            println!("{}", list_plugins());
            ExitCode::SUCCESS
        }
        "defaults" => {
            let doc = WindowPolicyDocument::compiled_defaults();
            println!("{}", serde_json::to_string_pretty(&doc).unwrap());
            ExitCode::SUCCESS
        }
        "digest" => {
            let doc = WindowPolicyDocument::compiled_defaults();
            println!("{}", doc.digest_hex());
            ExitCode::SUCCESS
        }
        "save-defaults" => match store.save_version(WindowPolicyDocument::compiled_defaults()) {
            Ok(p) => {
                println!("{}", serde_json::to_string_pretty(&p).unwrap());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        "activate" => {
            let Some(v) = args.next().and_then(|s| s.parse::<u64>().ok()) else {
                eprintln!("activate requires VERSION");
                return ExitCode::FAILURE;
            };
            match store.activate(v) {
                Ok(active) => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "ok": true,
                            "version": active.document.version,
                            "digest": active.digest_hex,
                            "source": "active_file",
                            "setups": active.document.setups,
                        })
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        "disable" => {
            let Some(ids_arg) = args.next() else {
                eprintln!("disable requires comma-separated SETUP_IDS");
                return ExitCode::FAILURE;
            };
            let active = match store.load_active() {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let mut doc = active.document;
            let mut changed = Vec::new();
            for id in ids_arg.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                match doc.setups.get_mut(id) {
                    Some(params) => {
                        params.enabled = false;
                        changed.push(id.to_string());
                    }
                    None => {
                        eprintln!("unknown setup_id: {id}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            doc.version = 0;
            doc.change_reason = format!(
                "disable repeat-negative setups (48h walk-forward): {}",
                changed.join(", ")
            );
            match store.save_version(doc) {
                Ok(pointer) => match store.activate(pointer.version) {
                    Ok(new_active) => {
                        println!(
                            "{}",
                            serde_json::json!({
                                "ok": true,
                                "version": new_active.document.version,
                                "digest": new_active.digest_hex,
                                "disabled": changed,
                                "change_reason": new_active.document.change_reason,
                            })
                        );
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("error activating: {e}");
                        ExitCode::FAILURE
                    }
                },
                Err(e) => {
                    eprintln!("error saving: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        "validate-defaults" => {
            let p = LateContinuationParams::compiled_defaults();
            let errors = p.validate();
            if errors.is_empty() {
                println!("{{\"ok\":true}}");
                ExitCode::SUCCESS
            } else {
                eprintln!("{errors:?}");
                ExitCode::FAILURE
            }
        }
        other => {
            eprintln!("unknown command: {other}");
            let _ = SetupPolicyBag::compiled_defaults();
            ExitCode::FAILURE
        }
    }
}

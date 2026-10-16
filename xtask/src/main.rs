//! `cargo xtask` — bundling, installing and validating MiteFX plugins.
//!
//!     cargo xtask bundle <package> --release   CLAP + VST3 bundles into target/bundled (nih_plug_xtask)
//!     cargo xtask install <package>            bundle (release), then copy into the user's plugin folders
//!     cargo xtask validate <package>           clap-validator + pluginval on the bundles
//!
//! Everything else is forwarded to nih_plug_xtask.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("install") => with_package(&args, install),
        Some("validate") => with_package(&args, validate),
        _ => nih_plug_xtask::main_with_args("cargo xtask", args).map_err(|e| e.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn with_package(args: &[String], f: fn(&str) -> Result<(), String>) -> Result<(), String> {
    let package = args.get(1).ok_or_else(|| format!("usage: cargo xtask {} <package>", args[0]))?;
    f(package)
}

/// Bundle names come from bundler.toml (`[mite_gain] name = "Mite Gain"`).
fn bundle_name(package: &str) -> Result<String, String> {
    let toml = std::fs::read_to_string(workspace_root().join("bundler.toml")).map_err(|e| e.to_string())?;
    let mut in_section = false;
    for line in toml.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line == format!("[{package}]");
        } else if in_section {
            if let Some(name) = line.strip_prefix("name").and_then(|r| r.trim_start().strip_prefix('=')) {
                return Ok(name.trim().trim_matches('"').to_owned());
            }
        }
    }
    Ok(package.to_owned())
}

fn bundles(package: &str) -> Result<(PathBuf, PathBuf), String> {
    let name = bundle_name(package)?;
    let dir = workspace_root().join("target/bundled");
    let clap = dir.join(format!("{name}.clap"));
    let vst3 = dir.join(format!("{name}.vst3"));
    if !clap.exists() || !vst3.exists() {
        return Err(format!("bundles not found in {}; run `cargo xtask bundle {package} --release` first", dir.display()));
    }
    Ok((clap, vst3))
}

fn install(package: &str) -> Result<(), String> {
    // Always from a fresh release bundle: copying whatever was bundled last installs stale code.
    let args = vec!["bundle".to_owned(), package.to_owned(), "--release".to_owned()];
    nih_plug_xtask::main_with_args("cargo xtask", args).map_err(|e| e.to_string())?;
    let (clap, vst3) = bundles(package)?;
    let (clap_dir, vst3_dir) = plugin_dirs()?;
    for (src, dir) in [(&clap, &clap_dir), (&vst3, &vst3_dir)] {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let dest = dir.join(src.file_name().expect("bundle has a name"));
        copy_recursive(src, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        println!("installed {}", dest.display());
    }
    Ok(())
}

fn validate(package: &str) -> Result<(), String> {
    let (clap, vst3) = bundles(package)?;
    let mut failures = Vec::new();
    let mut ran = 0;

    match Command::new("clap-validator").arg("validate").arg(&clap).status() {
        Ok(s) => {
            ran += 1;
            if !s.success() {
                failures.push("clap-validator");
            }
        }
        Err(_) => eprintln!("skipped: clap-validator not on PATH (cargo install --git https://github.com/free-audio/clap-validator)"),
    }
    match Command::new("pluginval")
        .args(["--strictness-level", "10", "--validate-in-process", "--validate"])
        .arg(&vst3)
        .status()
    {
        Ok(s) => {
            ran += 1;
            if !s.success() {
                failures.push("pluginval");
            }
        }
        Err(_) => eprintln!("skipped: pluginval not on PATH (https://github.com/Tracktion/pluginval/releases)"),
    }

    if ran == 0 {
        Err("no validators available".into())
    } else if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("failed: {}", failures.join(", ")))
    }
}

fn plugin_dirs() -> Result<(PathBuf, PathBuf), String> {
    if cfg!(target_os = "windows") {
        let common = std::env::var("COMMONPROGRAMFILES").map_err(|_| "COMMONPROGRAMFILES not set")?;
        Ok((Path::new(&common).join("CLAP"), Path::new(&common).join("VST3")))
    } else {
        let home = PathBuf::from(std::env::var("HOME").map_err(|_| "HOME not set")?);
        if cfg!(target_os = "macos") {
            let base = home.join("Library/Audio/Plug-Ins");
            Ok((base.join("CLAP"), base.join("VST3")))
        } else {
            Ok((home.join(".clap"), home.join(".vst3")))
        }
    }
}

fn copy_recursive(src: &Path, dest: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        if dest.exists() {
            std::fs::remove_dir_all(dest)?;
        }
        std::fs::create_dir_all(dest)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dest.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dest).map(|_| ())
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask lives in the workspace").to_path_buf()
}

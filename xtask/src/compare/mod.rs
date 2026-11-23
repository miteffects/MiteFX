//! `cargo xtask compare <eq|comp|limit> [--analyze-only]`: renders test signals through FabFilter's
//! plugin and ours in Reaper, measures both, and writes the results into
//! `docs/compare/pro-q-4.md`, `docs/compare/pro-c-3.md` or `docs/compare/pro-l-2.md`.
//!
//! Needs Reaper on PATH, the FabFilter plugins (on Linux through yabridge) and our installed
//! plugins (`cargo xtask install`). Work files go to `target/compare/`. Reaper runs off screen
//! when gamescope is installed (its headless backend); `MITEFX_COMPARE_VISIBLE=1` shows it.

mod analyze;
mod comp;
mod eq;
mod limit;
mod manifest;
mod reaper;
mod report;
mod signals;
mod wav;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Fab,
    Mite,
}

impl Vendor {
    fn tag(self) -> &'static str {
        match self {
            Self::Fab => "fab",
            Self::Mite => "mite",
        }
    }

    fn name(self, fab: &'static str, mite: &'static str) -> &'static str {
        match self {
            Self::Fab => fab,
            Self::Mite => mite,
        }
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let which = args.get(1).map(String::as_str).ok_or("usage: cargo xtask compare <eq|comp|limit> [--analyze-only]")?;
    let analyze_only = args.iter().any(|a| a == "--analyze-only");
    let root = super::workspace_root();
    let work = root.join("target/compare").join(which);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let (cases, doc) = match which {
        "eq" => (eq::cases(&work, &manifest::Manifest::load(&root.join("tests/golden/mite_eq.params.txt"))?)?, root.join("docs/compare/pro-q-4.md")),
        "comp" => (comp::cases(&work, &manifest::Manifest::load(&root.join("tests/golden/mite_comp.params.txt"))?)?, root.join("docs/compare/pro-c-3.md")),
        "limit" => (limit::cases(&work, &manifest::Manifest::load(&root.join("tests/golden/mite_limit.params.txt"))?)?, root.join("docs/compare/pro-l-2.md")),
        _ => return Err(format!("unknown comparison {which}: eq, comp or limit")),
    };
    let rendered = if analyze_only {
        reaper::load_log(&work)?
    } else {
        println!("rendering {} cases in Reaper…", cases.len());
        reaper::render(&cases, &work)?
    };
    let tables = match which {
        "eq" => eq::tables(&work, &rendered)?,
        "limit" => limit::tables(&work, &rendered)?,
        _ => comp::tables(&work, &rendered)?,
    };
    let stamp = std::process::Command::new("date").arg("+%F").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default();
    report::write(&doc, &tables, &stamp)?;
    println!("wrote {}", doc.display());
    Ok(())
}

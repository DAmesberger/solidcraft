//! `cargo xtask render-parity`: the viewport's parity frames (`crates/gpu/src/view/
//! parity_frames*.rs`) drawn by this tree and by an earlier revision, on this machine's GPU, and
//! compared. Standard frames must be byte-identical to the base; Realistic frames must be within
//! a tolerance of the realistic base (docs/material-graph.md §5.9, §6.3).
//!
//! An earlier revision gets this tree's harness: the tool checks it out in a temporary worktree,
//! copies the harness files in, includes them in `view.rs`'s `glass_tests` and dumps the frames
//! there. The Standard file only uses API upstream's renderer already had; the Realistic file
//! needs the product look and material graphs, so a realistic base must have both.

use std::path::{Path, PathBuf};
use std::process::Command;

const STANDARD: &str = "crates/gpu/src/view/parity_frames.rs";
const REALISTIC: &str = "crates/gpu/src/view/parity_frames_realistic.rs";
const VIEW: &str = "crates/gpu/src/view.rs";

struct Options {
    base: String,
    realistic_base: Option<String>,
    tolerance: u8,
    out: PathBuf,
    keep: bool,
}

fn options(root: &Path, args: &[&str]) -> Result<Options, String> {
    let arg = |name: &str| args.iter().position(|a| *a == name).and_then(|i| args.get(i + 1)).map(|s| (*s).to_string());
    let base = arg("--base").ok_or_else(|| "render-parity: --base REV is required (see `cargo xtask help`)".to_string())?;
    let tolerance = match arg("--tolerance") {
        Some(t) => t.parse().map_err(|_| format!("render-parity: --tolerance {t}: want 0–255"))?,
        None => 1,
    };
    let out = arg("--out").map_or_else(|| root.join("target/render-parity"), PathBuf::from);
    Ok(Options { base, realistic_base: arg("--realistic-base"), tolerance, out, keep: args.contains(&"--keep") })
}

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let o = options(root, args)?;
    let (ours, theirs) = target_dirs(root);
    let current = o.out.join("current");
    dump(root, &current, &ours)?;
    let mut failures = Vec::new();
    let mut revisions = vec![(o.base.clone(), o.realistic_base.as_deref() == Some(o.base.as_str()))];
    if let Some(r) = o.realistic_base.as_ref().filter(|r| **r != o.base) {
        revisions.push((r.clone(), true));
    }
    for (rev, realistic) in &revisions {
        let dir = o.out.join(format!("rev-{}", sanitize(rev)));
        dump_revision(root, rev, *realistic, &dir, &o.out, &theirs, o.keep)?;
        if *rev == o.base {
            failures.extend(compare(&current, &dir, "std-", 0)?);
        }
        if o.realistic_base.as_deref() == Some(rev.as_str()) {
            failures.extend(compare(&current, &dir, "real-", o.tolerance)?);
        }
    }
    if failures.is_empty() {
        eprintln!("render-parity: all frames match");
        Ok(())
    } else {
        Err(format!("render-parity: {}", failures.join("; ")))
    }
}

/// This tree's target directory, and the one the revisions build in. They must differ: Cargo
/// names a workspace member's artifacts by its path inside the workspace, so two checkouts
/// sharing one directory overwrite each other's build-script output.
fn target_dirs(root: &Path) -> (PathBuf, PathBuf) {
    let ours = match std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        Some(dir) => root.join(dir),
        None => root.join("target"),
    };
    let theirs = ours.join("render-parity");
    (ours, theirs)
}

fn sanitize(rev: &str) -> String {
    rev.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect()
}

/// Dump the parity frames of the tree at `tree` into `out` (emptied first).
fn dump(tree: &Path, out: &Path, target: &Path) -> Result<(), String> {
    if out.exists() {
        std::fs::remove_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(tree).env("CARGO_TARGET_DIR", target).env("PARITY_OUT", out).args([
        "test",
        "-p",
        "solidcraft-gpu",
        "--lib",
        "parity_frames",
        "--",
        "--ignored",
        "--test-threads",
        "1",
    ]);
    eprintln!("$ (in {}) cargo test -p solidcraft-gpu --lib parity_frames -- --ignored", tree.display());
    let status = c.status().map_err(|e| format!("cargo test: {e}"))?;
    if !status.success() {
        return Err(format!("parity frames in {}: cargo test exited with {status}", tree.display()));
    }
    let n = std::fs::read_dir(out).map_err(|e| format!("{}: {e}", out.display()))?.count();
    eprintln!("render-parity: {n} frames from {}", tree.display());
    Ok(())
}

/// Check `rev` out in a temporary worktree, give it this tree's harness and dump its frames.
fn dump_revision(root: &Path, rev: &str, realistic: bool, out: &Path, work: &Path, target: &Path, keep: bool) -> Result<(), String> {
    let tree = work.join(format!("tree-{}", sanitize(rev)));
    if tree.exists() {
        let _ = git(root, &["worktree", "remove", "--force", &tree.to_string_lossy()]);
        let _ = std::fs::remove_dir_all(&tree);
    }
    git(root, &["worktree", "add", "--detach", &tree.to_string_lossy(), rev])?;
    let result = (|| {
        let mut files = vec![STANDARD];
        if realistic {
            files.push(REALISTIC);
        }
        for file in &files {
            std::fs::copy(root.join(file), tree.join(file)).map_err(|e| format!("{file} into {rev}: {e}"))?;
        }
        include_harness(&tree.join(VIEW), realistic)?;
        dump(&tree, out, target)
    })();
    if !keep {
        let _ = git(root, &["worktree", "remove", "--force", &tree.to_string_lossy()]);
    }
    result
}

/// Include the harness in `view.rs`'s `glass_tests` (unless it already is).
fn include_harness(view: &Path, realistic: bool) -> Result<(), String> {
    let text = std::fs::read_to_string(view).map_err(|e| format!("{}: {e}", view.display()))?;
    let mut modules = String::new();
    if !text.contains("include!(\"view/parity_frames.rs\")") {
        modules.push_str("    mod parity_frames {\n        include!(\"view/parity_frames.rs\");\n    }\n");
    }
    if realistic && !text.contains("include!(\"view/parity_frames_realistic.rs\")") {
        modules.push_str("    mod parity_frames_realistic {\n        include!(\"view/parity_frames_realistic.rs\");\n    }\n");
    }
    if modules.is_empty() {
        return Ok(());
    }
    let Some(at) = text.find("mod glass_tests {\n").map(|i| i + "mod glass_tests {\n".len()) else {
        return Err(format!("{}: no `mod glass_tests` to include the harness in", view.display()));
    };
    let patched = format!("{}{modules}{}", &text[..at], &text[at..]);
    std::fs::write(view, patched).map_err(|e| format!("{}: {e}", view.display()))
}

fn git(root: &Path, args: &[&str]) -> Result<(), String> {
    let status = Command::new("git").current_dir(root).args(args).status().map_err(|e| format!("git: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("git {}: exited with {status}", args.join(" "))) }
}

/// Compare the frames named `prefix…` in `current` and `base`: each channel within `tolerance`.
/// Returns the failures (empty when all match).
fn compare(current: &Path, base: &Path, prefix: &str, tolerance: u8) -> Result<Vec<String>, String> {
    let names = |dir: &Path| -> Result<std::collections::BTreeSet<String>, String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(entries.filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n.starts_with(prefix) && n.ends_with(".rgba")).collect())
    };
    let (ours, theirs) = (names(current)?, names(base)?);
    let mut failures = Vec::new();
    let missing: Vec<_> = ours.symmetric_difference(&theirs).cloned().collect();
    if !missing.is_empty() {
        failures.push(format!("{} {prefix}frames only on one side (first: {})", missing.len(), missing[0]));
    }
    let (mut identical, mut within, mut worst) = (0usize, 0usize, 0u8);
    let mut over = Vec::new();
    for name in ours.intersection(&theirs) {
        let a = std::fs::read(current.join(name)).map_err(|e| format!("{name}: {e}"))?;
        let b = std::fs::read(base.join(name)).map_err(|e| format!("{name}: {e}"))?;
        if a == b {
            identical += 1;
            continue;
        }
        let step = if a.len() == b.len() { a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0) } else { u8::MAX };
        let pixels = a.chunks(4).zip(b.chunks(4)).filter(|(x, y)| x != y).count();
        worst = worst.max(step);
        if step <= tolerance {
            within += 1;
        } else {
            over.push(format!("{name} ({pixels} pixels, step {step})"));
        }
    }
    let compared = ours.intersection(&theirs).count();
    eprintln!(
        "render-parity {prefix}: {compared} frames compared with {}: {identical} identical, {within} within {tolerance}, {} over; largest step {worst}",
        base.display(),
        over.len()
    );
    for line in over.iter().take(20) {
        eprintln!("  {line}");
    }
    if !over.is_empty() {
        failures.push(format!("{} {prefix}frames differ by more than {tolerance}", over.len()));
    }
    if compared == 0 {
        failures.push(format!("no {prefix}frames to compare"));
    }
    Ok(failures)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-render-parity-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        dir
    }

    #[test]
    fn standard_frames_must_be_identical_and_realistic_within_the_tolerance() {
        let (a, b) = (temp("a"), temp("b"));
        for (dir, std_byte, real_byte) in [(&a, 10u8, 100u8), (&b, 10, 101)] {
            std::fs::write(dir.join("std-x.rgba"), [std_byte; 16]).unwrap_or_else(|e| panic!("{e}"));
            std::fs::write(dir.join("real-x.rgba"), [real_byte; 16]).unwrap_or_else(|e| panic!("{e}"));
        }
        assert!(compare(&a, &b, "std-", 0).unwrap_or_else(|e| panic!("{e}")).is_empty());
        assert!(compare(&a, &b, "real-", 1).unwrap_or_else(|e| panic!("{e}")).is_empty());
        assert_eq!(compare(&a, &b, "real-", 0).unwrap_or_else(|e| panic!("{e}")).len(), 1, "one step over a zero tolerance fails");
        std::fs::write(b.join("std-x.rgba"), [11u8; 16]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(compare(&a, &b, "std-", 0).unwrap_or_else(|e| panic!("{e}")).len(), 1, "a Standard frame one step off fails");
        std::fs::write(a.join("std-y.rgba"), [0u8; 16]).unwrap_or_else(|e| panic!("{e}"));
        let failures = compare(&a, &b, "std-", 0).unwrap_or_else(|e| panic!("{e}"));
        assert!(failures.iter().any(|f| f.contains("only on one side")), "{failures:?}");
    }

    #[test]
    fn the_harness_is_included_in_glass_tests_once() {
        let dir = temp("view");
        let view = dir.join("view.rs");
        std::fs::write(&view, "mod x {}\n#[cfg(test)]\nmod glass_tests {\n    use super::*;\n}\n").unwrap_or_else(|e| panic!("{e}"));
        include_harness(&view, true).unwrap_or_else(|e| panic!("{e}"));
        include_harness(&view, true).unwrap_or_else(|e| panic!("{e}"));
        let text = std::fs::read_to_string(&view).unwrap_or_default();
        assert_eq!(text.matches("include!(\"view/parity_frames.rs\")").count(), 1, "{text}");
        assert_eq!(text.matches("include!(\"view/parity_frames_realistic.rs\")").count(), 1, "{text}");
        assert!(text.find("mod glass_tests {").unwrap_or(usize::MAX) < text.find("mod parity_frames").unwrap_or(0), "{text}");
        std::fs::write(&view, "mod tests {}\n").unwrap_or_else(|e| panic!("{e}"));
        assert!(include_harness(&view, false).is_err(), "a view.rs without glass_tests is refused");
    }
}

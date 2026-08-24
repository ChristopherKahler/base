//! Multimodal ingest for `base graph extract` (P4). Turns non-markdown sources —
//! PDF, image, audio, video — into plain text that flows through the SAME concept
//! extractor as markdown. PDF → `pdftotext`; audio/video → Whisper transcription;
//! image → a Claude vision pass (description text).
//!
//! Dependency policy (the whole point of the off-by-default switch): base ships
//! with multimodal OFF, so a stock install pulls NONE of these tools. The first
//! time extract runs over a non-markdown corpus *with multimodal enabled*, base
//! bootstraps exactly the tools that corpus needs — ONCE (marker-gated), never
//! again. An operator who never flips the switch never pays for poppler / ffmpeg
//! / whisper.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Modality {
    Markdown,
    Pdf,
    Image,
    Audio,
    Video,
}

impl Modality {
    pub fn label(self) -> &'static str {
        match self {
            Modality::Markdown => "markdown",
            Modality::Pdf => "pdf",
            Modality::Image => "image",
            Modality::Audio => "audio",
            Modality::Video => "video",
        }
    }
}

/// Classify a file by extension. `None` = not an ingestible source.
pub fn classify(path: &Path) -> Option<Modality> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    Some(match ext.as_str() {
        "md" | "markdown" => Modality::Markdown,
        "pdf" => Modality::Pdf,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => Modality::Image,
        "mp3" | "wav" | "m4a" | "flac" | "ogg" | "aac" => Modality::Audio,
        "mp4" | "mov" | "mkv" | "webm" | "avi" => Modality::Video,
        _ => return None,
    })
}

// ─── Dependency bootstrap (NO sudo) ──────────────────────────

/// An external tool a modality needs. Install hierarchy is in-process > user-space
/// > system — so every dep here installs WITHOUT sudo (pip --user / pipx). PDF is
/// not a Dep at all: it's handled in-process by the `pdf-extract` crate.
struct Dep {
    bin: &'static str,
    /// PyPI package that provides `bin` (installed with `pip install --user`).
    pip: &'static str,
    /// Human fallback shown if the no-sudo auto-install can't complete.
    manual: &'static str,
}

const FFMPEG: Dep = Dep {
    bin: "ffmpeg",
    pip: "imageio-ffmpeg",
    manual: "pip install --user imageio-ffmpeg   (or your OS package: brew install ffmpeg / apt install ffmpeg)",
};
const WHISPER: Dep = Dep {
    bin: "whisper",
    pip: "openai-whisper",
    manual: "pipx install openai-whisper   (or: pip install --user openai-whisper)",
};

/// Tools a given modality needs at runtime. Markdown/image/PDF need none:
/// markdown is plain read, image uses the already-present `claude` vision path,
/// PDF is extracted in-process by the `pdf-extract` crate.
fn deps_for(m: Modality) -> &'static [Dep] {
    match m {
        Modality::Audio | Modality::Video => &[FFMPEG, WHISPER],
        Modality::Markdown | Modality::Image | Modality::Pdf => &[],
    }
}

fn marker_path() -> Option<PathBuf> {
    Some(crate::home::home_root()?.join(".base-gbl").join(".multimodal-bootstrap"))
}

/// base-managed bin dir on which user-space binaries (e.g. an imageio-ffmpeg
/// shim) are exposed so child tools like whisper can find them.
fn base_bin_dir() -> Option<PathBuf> {
    Some(crate::home::home_root()?.join(".base-gbl").join("bin"))
}

/// Resolve the Python interpreter name to invoke. On Unix `python3` is canonical
/// (`python` may still be Python 2); on Windows the python.org installer ships
/// `python.exe` only and a bare `python3` hits the Microsoft Store stub, so prefer
/// `python` there. Falls back to the other name when the preferred one isn't found.
pub fn python_bin() -> &'static str {
    #[cfg(windows)]
    let (first, second) = ("python", "python3");
    #[cfg(not(windows))]
    let (first, second) = ("python3", "python");
    if on_path(first) {
        first
    } else if on_path(second) {
        second
    } else {
        first
    }
}

/// Is a binary resolvable? Side-effect-free scan of PATH plus base's own bin dir
/// (don't spawn the tool just to test for it).
fn on_path(bin: &str) -> bool {
    if let Some(d) = base_bin_dir() {
        let c = d.join(bin);
        if c.is_file() || c.with_extension("exe").is_file() {
            return true;
        }
    }
    let Some(paths) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&paths).any(|dir| {
        let cand = dir.join(bin);
        cand.is_file() || cand.with_extension("exe").is_file()
    })
}

fn pip_cmd() -> Option<Command> {
    if on_path("pipx") {
        return Some(Command::new("pipx"));
    }
    for p in ["pip3", "pip"] {
        if on_path(p) {
            return Some(Command::new(p));
        }
    }
    None
}

/// Auto-install a single dep WITHOUT sudo. Returns whether it likely succeeded
/// (presence is re-verified by the caller regardless).
fn try_install(dep: &Dep) -> bool {
    if dep.bin == "ffmpeg" {
        return install_ffmpeg_userspace();
    }
    let Some(mut cmd) = pip_cmd() else {
        eprintln!("  ! no pipx/pip found to install {}", dep.bin);
        return false;
    };
    // pipx uses `install <pkg>`; pip needs the `--user` flag.
    let prog = cmd.get_program().to_string_lossy().to_string();
    cmd.arg("install");
    if prog != "pipx" {
        cmd.arg("--user");
    }
    cmd.arg(dep.pip);
    eprintln!("  → installing {} (no sudo) …", dep.bin);
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

/// Install ffmpeg without sudo: pip-install `imageio-ffmpeg` (ships a static
/// ffmpeg binary) and symlink it into base's bin dir as `ffmpeg`, so whisper —
/// which shells out to `ffmpeg` by name — finds it via the PATH we set.
fn install_ffmpeg_userspace() -> bool {
    let Some(mut cmd) = pip_cmd() else {
        eprintln!("  ! no pipx/pip found to install ffmpeg");
        return false;
    };
    let prog = cmd.get_program().to_string_lossy().to_string();
    cmd.arg("install");
    if prog != "pipx" {
        cmd.arg("--user");
    }
    cmd.arg("imageio-ffmpeg");
    eprintln!("  → installing ffmpeg via imageio-ffmpeg (no sudo) …");
    if !cmd.status().map(|s| s.success()).unwrap_or(false) {
        return false;
    }
    // Resolve the bundled binary and shim it onto base's bin dir.
    let py = python_bin();
    let out = Command::new(py)
        .arg("-c")
        .arg("import imageio_ffmpeg,sys; sys.stdout.write(imageio_ffmpeg.get_ffmpeg_exe())")
        .output();
    let Ok(out) = out else { return false };
    if !out.status.success() {
        return false;
    }
    let exe = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if exe.is_empty() {
        return false;
    }
    let Some(bin_dir) = base_bin_dir() else { return false };
    let _ = std::fs::create_dir_all(&bin_dir);
    let link = bin_dir.join("ffmpeg");
    let _ = std::fs::remove_file(&link);
    #[cfg(unix)]
    let ok = std::os::unix::fs::symlink(&exe, &link).is_ok();
    #[cfg(not(unix))]
    let ok = std::fs::copy(&exe, &link).is_ok();
    ok
}

/// Ensure every tool the given modalities need is present. Re-verifies presence
/// every call (cheap PATH scan); the one-time AUTO-INSTALL attempt is marker-gated
/// so a box that can't auto-install isn't nagged on every run — it gets the manual
/// command once and base trusts the operator to run it.
pub fn ensure_deps(mods: &BTreeSet<Modality>) -> Result<()> {
    let mut needed: Vec<&Dep> = Vec::new();
    for &m in mods {
        for d in deps_for(m) {
            if !needed.iter().any(|x| x.bin == d.bin) {
                needed.push(d);
            }
        }
    }
    let missing: Vec<&Dep> = needed.into_iter().filter(|d| !on_path(d.bin)).collect();
    if missing.is_empty() {
        // Nothing to install — don't write the marker. The marker means "we
        // attempted a bootstrap", so a dep that goes missing later still gets
        // its one auto-install attempt.
        return Ok(());
    }

    let marker = marker_path();
    let already_bootstrapped = marker.as_ref().map(|p| p.exists()).unwrap_or(false);

    let names: Vec<&str> = missing.iter().map(|d| d.bin).collect();
    let manual: Vec<String> = missing.iter().map(|d| format!("    {}", d.manual)).collect();

    if already_bootstrapped {
        bail!(
            "multimodal dependencies still missing: {}.\nInstall them, then retry:\n{}",
            names.join(", "),
            manual.join("\n")
        );
    }

    eprintln!(
        "First multimodal run — bootstrapping one-time dependencies: {} (this happens once)",
        names.join(", ")
    );
    for dep in &missing {
        try_install(dep);
    }
    // Record the attempt so we never auto-install again, even if it failed.
    if let Some(mp) = &marker {
        if let Some(parent) = mp.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(mp, b"ok\n");
    }

    let still: Vec<&str> = missing.iter().filter(|d| !on_path(d.bin)).map(|d| d.bin).collect();
    if still.is_empty() {
        eprintln!("✓ multimodal dependencies ready.");
        Ok(())
    } else {
        bail!(
            "could not auto-install: {}.\nInstall manually, then retry:\n{}",
            still.join(", "),
            manual.join("\n")
        );
    }
}

// ─── Per-modality text extraction ────────────────────────────

/// Turn a non-markdown source into plain text for the concept extractor. Deps are
/// assumed already ensured by the caller (`ensure_deps`) so this stays a pure
/// transform.
pub fn extract_text(path: &Path, modality: Modality, model: Option<&str>) -> Result<String> {
    match modality {
        Modality::Markdown => Ok(std::fs::read_to_string(path).unwrap_or_default()),
        Modality::Pdf => pdf_to_text(path),
        Modality::Image => image_to_text(path, model),
        Modality::Audio | Modality::Video => transcribe(path),
    }
}

/// In-process PDF text extraction via the `pdf-extract` crate — no external tool,
/// no poppler, no sudo. Self-contained in the base binary.
fn pdf_to_text(path: &Path) -> Result<String> {
    pdf_extract::extract_text(path)
        .with_context(|| format!("extracting text from PDF {}", path.display()))
}

const VISION_PROMPT: &str =
    "Describe this image as knowledge-graph source material. Capture every readable \
     piece of text verbatim, plus diagrams, entities, labels, and the relationships \
     between them. Write dense factual prose — no preamble, no 'this image shows'.";

fn image_to_text(path: &Path, model: Option<&str>) -> Result<String> {
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    crate::llm::complete_with_image(VISION_PROMPT, &abs, model)
}

/// Whisper handles its own ffmpeg decode, so a video file can be transcribed
/// directly — no manual audio extraction step. Output goes to a temp dir as a
/// `.txt` we read back (cleaner than parsing timestamped stdout).
fn transcribe(path: &Path) -> Result<String> {
    let tmp = std::env::temp_dir().join(format!("base-mm-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).context("creating transcription temp dir")?;

    // Force CPU: most operators don't have a correctly-built CUDA stack, and a
    // GPU/kernel mismatch is a hard runtime failure. Transcription is identical on
    // CPU (just slower) and this path is cached, so CPU is the robust default.
    let mut cmd = Command::new("whisper");
    cmd.arg(path)
        .arg("--model").arg("base")
        .arg("--device").arg("cpu")
        .arg("--fp16").arg("False")
        .arg("--output_format").arg("txt")
        .arg("--output_dir").arg(&tmp)
        .arg("--verbose").arg("False");
    // Ensure whisper's `ffmpeg` lookup finds a base-managed (imageio-ffmpeg) shim.
    if let Some(bin_dir) = base_bin_dir() {
        let path_var = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}:{}", bin_dir.display(), path_var));
    }
    let status = cmd.status().context("spawning whisper")?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        bail!("whisper transcription failed for {}", path.display());
    }

    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("audio");
    let txt = tmp.join(format!("{stem}.txt"));
    let text = std::fs::read_to_string(&txt)
        .with_context(|| format!("reading whisper output {}", txt.display()))?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(text)
}

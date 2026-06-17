use anyhow::{anyhow, bail, Context, Result};
use chrono::Local;
use serde::Serialize;
use std::collections::HashSet;
use std::env::set_current_dir;
use std::fs;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, RecvTimeoutError, Sender},
    Arc,
};
use std::thread;
use std::time::Duration;
use tracing::{info, warn};

use crate::window_util::CreateNoWindow as _;
use rust_i18n::t;

#[cfg(target_os = "macos")]
const WORKSPACE_DIR_NAME: &str = "NTEPilot";
const BACKEND_ENTRYPOINT: &str = "main.py";
const PYPROJECT_FILE: &str = "pyproject.toml";
const UV_LOCK_FILE: &str = "uv.lock";
const MAX_SYNC_RETRIES: usize = 20;
const RETRY_DELAY: Duration = Duration::from_secs(1);
const CLEANUP_RETRIES: usize = 20;
const PYTHON_VERSION: &str = "3.14.3";
const DEFAULT_UV_PYTHON_INSTALL_MIRRORS: &[&str] = &[
    "https://registry.npmmirror.com/-/binary/python-build-standalone/",
    "https://mirror.nju.edu.cn/github-release/astral-sh/python-build-standalone/",
    "https://python-standalone.org/mirror/astral-sh/python-build-standalone/",
];
const BOOTSTRAP_UV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bootstrap_uv.bin"));

#[derive(Clone, Debug, Serialize)]
pub struct SplashUpdate {
    pub subtitle: String,
    pub title: String,
    pub detail: String,
    pub progress: u8,
    pub is_error: bool,
}

impl SplashUpdate {
    pub fn loading(title: impl Into<String>, detail: impl Into<String>, progress: u8) -> Self {
        Self {
            subtitle: t!("setup.connecting").to_string(),
            title: title.into(),
            detail: detail.into(),
            progress: progress.min(100),
            is_error: false,
        }
    }

    pub fn error(title: impl Into<String>, detail: impl Into<String>, progress: u8) -> Self {
        Self {
            subtitle: t!("setup.connection_failed").to_string(),
            title: title.into(),
            detail: detail.into(),
            progress: progress.min(100),
            is_error: true,
        }
    }

    pub fn with_subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = subtitle.into();
        self
    }
}

const TIPS_COUNT: usize = 19;

pub fn get_tip() -> String {
    let now = Local::now().timestamp() as usize;
    let idx = now % TIPS_COUNT;
    let key = format!("tips.{idx}");
    t!(&key).to_string()
}

fn backend_workspace_has_required_files(path: &Path) -> bool {
    path.join(BACKEND_ENTRYPOINT).is_file()
        && path.join(PYPROJECT_FILE).is_file()
        && path.join(UV_LOCK_FILE).is_file()
}

fn executable_dir() -> Result<PathBuf> {
    Ok(std::env::current_exe()?
        .parent()
        .ok_or_else(|| anyhow!(t!("errors.launcher_dir_not_found")))?
        .to_path_buf())
}

fn workspace_candidates(exe_dir: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    #[cfg(target_os = "macos")]
    {
        use std::ffi::OsStr;
        if exe_dir.file_name() == Some(OsStr::new("MacOS")) {
            if let Some(contents_dir) = exe_dir.parent() {
                candidates.push(contents_dir.join(WORKSPACE_DIR_NAME));
            }
        }
    }

    candidates.push(exe_dir.to_path_buf());
    candidates
}

fn workspace_dir() -> Result<PathBuf> {
    let exe_dir = executable_dir()?;
    for candidate in workspace_candidates(&exe_dir) {
        if backend_workspace_has_required_files(&candidate) {
            return Ok(candidate);
        }
    }

    bail!(
        "Cannot find NTEPilot workspace. Expected {}, {}, and {} next to the launcher{}.",
        BACKEND_ENTRYPOINT,
        PYPROJECT_FILE,
        UV_LOCK_FILE,
        if cfg!(target_os = "macos") {
            " or in NTEP Launcher.app/Contents/NTEPilot"
        } else {
            ""
        }
    )
}

fn prepend_path_to_env(key: &str, path: PathBuf) {
    let mut paths = Vec::new();
    paths.push(path);
    if let Some(ref old_path) = &std::env::var_os(key) {
        paths.extend(std::env::split_paths(old_path));
    }
    std::env::set_var(key, std::env::join_paths(paths).unwrap());
}

fn venv_dir() -> PathBuf {
    workspace_dir()
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(".venv")
}

fn venv_bin_dir() -> PathBuf {
    let venv = venv_dir();
    if cfg!(windows) {
        venv.join("Scripts")
    } else {
        venv.join("bin")
    }
}

pub fn venv_python() -> PathBuf {
    venv_bin_dir().join(if cfg!(windows) {
        "python.exe"
    } else {
        "python"
    })
}

fn venv_python_install_dir() -> PathBuf {
    venv_dir().join("python")
}

fn venv_uv() -> PathBuf {
    venv_bin_dir().join(if cfg!(windows) { "uv.exe" } else { "uv" })
}

fn bootstrap_uv_path() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("ntep-bootstrap-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    let path = dir.join(if cfg!(windows) { "uv.exe" } else { "uv" });
    if !path.exists() {
        if BOOTSTRAP_UV.is_empty() {
            if let Some(path_uv) = std::env::var_os("UV").map(PathBuf::from) {
                return Ok(path_uv);
            }
            if let Some(path_uv) = find_on_path("uv") {
                return Ok(path_uv);
            }
            bail!(t!("errors.uv_not_found"));
        }
        fs::write(&path, BOOTSTRAP_UV)
            .with_context(|| t!("errors.write_uv_failed", path = path.display().to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&path)?.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions)?;
        }
    }
    Ok(path)
}

fn find_on_path(executable: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(executable);
        if candidate.exists() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let candidate = dir.join(format!("{executable}.exe"));
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn setup_environment() -> Result<()> {
    let dir = workspace_dir()?;
    info!("NTEPilot workspace is {:?}", &dir);
    set_current_dir(&dir)?;
    prepend_path_to_env("PATH", venv_bin_dir());
    Ok(())
}

pub fn setup_workspace(
    mut status_updater: impl FnMut(SplashUpdate),
    cancel_requested: Arc<AtomicBool>,
) -> Result<()> {
    info!("Starting setup for NTEPilot workspace...");
    status_updater(
        SplashUpdate::loading(
            t!("setup.preparing_workspace"),
            t!("setup.cleaning_cache"),
            8,
        )
        .with_subtitle(t!("setup.checking_env", tip = get_tip())),
    );
    let workspace = workspace_dir()?;
    if !backend_workspace_has_required_files(&workspace) {
        bail!("NTEPilot workspace is missing main.py, pyproject.toml, or uv.lock");
    }

    let bootstrap_uv = bootstrap_uv_path()?;
    ensure_runtime_tools(&bootstrap_uv, &cancel_requested, &mut status_updater)?;
    status_updater(
        SplashUpdate::loading(t!("setup.installing_deps"), t!("setup.verifying_deps"), 64)
            .with_subtitle(t!("setup.syncing_deps", tip = get_tip())),
    );
    uv_sync_project(&mut status_updater, &bootstrap_uv, &cancel_requested)?;
    status_updater(
        SplashUpdate::loading(t!("setup.finishing"), t!("setup.ready_to_launch"), 94)
            .with_subtitle(t!("setup.launching", tip = get_tip())),
    );
    Ok(())
}

pub fn cleanup_runtime_for_rebuild() -> Result<()> {
    let workspace = workspace_dir()?;
    let current_exe = std::env::current_exe()?;
    let workspace = workspace.canonicalize()?;
    let exe_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow!(t!("errors.launcher_dir_not_found")))?
        .canonicalize()?;
    if !cleanup_target_belongs_to_launcher(&workspace, &exe_dir) {
        bail!(t!(
            "errors.refuse_cleanup",
            actual = workspace.display().to_string(),
            expected = exe_dir.display().to_string()
        ));
    }

    kill_runtime_processes(&workspace);
    clean_uv_cache()?;

    let venv = workspace.join(".venv");
    if venv.exists() {
        info!("Removing {}", venv.display());
        remove_runtime_entry_with_retry(&venv).with_context(|| {
            t!(
                "errors.partial_cleanup_failed",
                errors = venv.display().to_string()
            )
        })?;
    }

    Ok(())
}

fn clean_uv_cache() -> Result<()> {
    let uv = bootstrap_uv_path()?;
    info!("Cleaning uv cache with {}", uv.display());
    let status = Command::new(&uv)
        .args(["cache", "clean"])
        .env("UV_NO_PROGRESS", "1")
        .create_no_window()
        .status()
        .with_context(|| {
            t!(
                "errors.uv_cache_cleanup_failed",
                error = uv.display().to_string()
            )
        })?;
    if !status.success() {
        bail!(t!("errors.uv_cache_failed"));
    }
    Ok(())
}

fn kill_runtime_processes(workspace: &Path) {
    let current_pid = std::process::id();
    let sys = sysinfo::System::new_all();
    for (pid, process) in sys.processes() {
        if pid.as_u32() == current_pid {
            continue;
        }

        let should_kill = process
            .exe()
            .map(|exe| path_is_inside(exe, workspace))
            .unwrap_or(false)
            || process
                .cwd()
                .map(|cwd| path_is_inside(cwd, workspace))
                .unwrap_or(false);

        if should_kill {
            info!(
                "Killing runtime process {} ({}) before cleanup",
                pid,
                process.name().to_string_lossy()
            );
            if !process.kill() {
                warn!("Failed to kill runtime process {}", pid);
            }
        }
    }

    thread::sleep(Duration::from_millis(500));
}

fn path_is_inside(path: &Path, parent: &Path) -> bool {
    path.canonicalize()
        .map(|path| path.starts_with(parent))
        .unwrap_or(false)
}

fn cleanup_target_belongs_to_launcher(workspace: &Path, exe_dir: &Path) -> bool {
    if workspace == exe_dir {
        return true;
    }

    #[cfg(target_os = "macos")]
    {
        let Some(contents_dir) = exe_dir.parent() else {
            return false;
        };
        let expected_workspace = contents_dir.join(WORKSPACE_DIR_NAME);
        return exe_dir.file_name() == Some(std::ffi::OsStr::new("MacOS"))
            && workspace == expected_workspace;
    }

    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn remove_runtime_entry(path: &Path) -> Result<()> {
    clear_readonly(path)?;
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            remove_runtime_entry(&entry?.path())?;
        }
        fs::remove_dir(path).with_context(|| {
            t!(
                "errors.delete_dir_failed",
                error = path.display().to_string()
            )
        })?;
    } else {
        fs::remove_file(path).with_context(|| {
            t!(
                "errors.delete_file_failed",
                error = path.display().to_string()
            )
        })?;
    }
    Ok(())
}

fn remove_runtime_entry_with_retry(path: &Path) -> Result<()> {
    let mut last_error = None;
    for attempt in 0..CLEANUP_RETRIES {
        match remove_runtime_entry(path) {
            Ok(()) => return Ok(()),
            Err(err) => {
                last_error = Some(err);
                if !path.exists() {
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(250 + attempt as u64 * 100));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        anyhow!(t!(
            "errors.delete_failed",
            error = path.display().to_string()
        ))
    }))
}

fn clear_readonly(path: &Path) -> Result<()> {
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(());
    };
    let mut permissions = metadata.permissions();
    if permissions.readonly() {
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)
            .with_context(|| t!("errors.chmod_failed", error = path.display().to_string()))?;
    }
    Ok(())
}

fn pipe_lines(read: impl Read + Send + 'static, tx: Sender<(bool, String)>, is_err: bool) {
    thread::spawn(move || {
        let mut reader = BufReader::new(read);
        let mut buffer = "".to_owned();
        loop {
            let mut line = [0u8; 64];
            match reader.read(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(size) => {
                    for c in &line[0..size] {
                        if *c < 32 || *c > 127 {
                            if !buffer.is_empty() {
                                let _ = tx.send((is_err, buffer));
                                buffer = "".to_owned();
                            }
                        } else if *c as char == ':' {
                            let mut cut = 0usize;
                            if let Some((l, r)) = buffer.split_once(':') {
                                if r.ends_with(l) {
                                    cut = r.len() + 1;
                                }
                            }
                            if cut > 0 {
                                let (l, r) = buffer.split_at(cut);
                                let _ = tx.send((is_err, l.to_owned()));
                                buffer = r.to_owned();
                            }
                            buffer.push(*c as char);
                        } else {
                            buffer.push(*c as char);
                        }
                    }
                }
            }
        }
        if !buffer.is_empty() {
            let _ = tx.send((is_err, buffer));
        }
    });
}

fn run_command(
    cmd: &mut Command,
    mut status_updater: impl FnMut(SplashUpdate),
    cancel_requested: &AtomicBool,
) -> Result<()> {
    let mut child = cmd
        .create_no_window()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let (tx, rx) = mpsc::channel::<(bool, String)>();
    if let Some(stdout) = child.stdout.take() {
        pipe_lines(stdout, tx.clone(), false);
    }
    if let Some(stderr) = child.stderr.take() {
        pipe_lines(stderr, tx.clone(), true);
    }
    drop(tx);

    let mut last_err = "".to_owned();
    let mut seen_packages = HashSet::new();
    let mut dependency_progress = 64u8;
    let mut dependency_elapsed_secs = 0u16;

    loop {
        if cancel_requested.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            bail!(t!("setup.cancel_cleaning"));
        }
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok((is_err, line)) => {
                if let Some(mut update) =
                    splash_update_for_dependency_output(&line, &mut seen_packages)
                {
                    update.progress = update.progress.max(dependency_progress);
                    dependency_progress = update.progress;
                    status_updater(update);
                }

                if is_err {
                    if is_uv_progress_line(&line) {
                        info!("{line}");
                    } else {
                        warn!("{line}");
                        last_err = line;
                    }
                } else {
                    info!("{line}");
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                dependency_elapsed_secs = dependency_elapsed_secs.saturating_add(1);
                let update = dependency_wait_update(dependency_elapsed_secs, dependency_progress);
                dependency_progress = update.progress;
                status_updater(update);
            }
            Err(RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    let status = child.wait()?;
    if !status.success() {
        if last_err.is_empty() {
            last_err = t!("setup.deps_failed").to_string();
        }
        return Err(anyhow!(last_err));
    }
    Ok(())
}

fn run_command_with_retry(
    build_cmd: impl Fn() -> Command,
    mut status_updater: impl FnMut(SplashUpdate),
    cancel_requested: &AtomicBool,
) -> Result<()> {
    for retry in 0..=MAX_SYNC_RETRIES {
        if cancel_requested.load(Ordering::SeqCst) {
            bail!(t!("setup.cancel_cleaning"));
        }

        match run_command(&mut build_cmd(), &mut status_updater, cancel_requested) {
            Ok(()) => return Ok(()),
            Err(err) => {
                if retry == MAX_SYNC_RETRIES {
                    return Err(err);
                }

                let retry_count = retry + 1;
                let error_text = err.to_string();
                warn!(
                    "{} failed (retry {retry_count}/{MAX_SYNC_RETRIES}): {error_text}",
                    t!("setup.deps_sync")
                );
                status_updater(splash_retry_update(retry_count, &error_text));
                thread::sleep(RETRY_DELAY);
            }
        }
    }

    unreachable!()
}

fn run_status_command(
    cmd: &mut Command,
    cancel_requested: &AtomicBool,
) -> Result<std::process::ExitStatus> {
    run_status_command_with_tick(cmd, cancel_requested, || {})
}

fn run_status_command_with_tick(
    cmd: &mut Command,
    cancel_requested: &AtomicBool,
    mut on_tick: impl FnMut(),
) -> Result<std::process::ExitStatus> {
    let mut child = cmd.create_no_window().spawn()?;
    loop {
        if cancel_requested.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            bail!(t!("setup.cancel_cleaning"));
        }

        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        on_tick();
        thread::sleep(Duration::from_millis(100));
    }
}

fn splash_retry_update(retry_count: usize, error_text: &str) -> SplashUpdate {
    let detail = t!(
        "setup.retry_detail",
        count = retry_count.to_string(),
        max = MAX_SYNC_RETRIES.to_string(),
        error = error_text
    );
    SplashUpdate::loading(t!("setup.retrying_deps"), detail, 64)
        .with_subtitle(t!("setup.syncing_deps", tip = get_tip()))
}

fn dependency_wait_update(elapsed_secs: u16, current_progress: u8) -> SplashUpdate {
    let synthetic_progress = (64 + (elapsed_secs / 4) as u8).min(89);
    let progress = current_progress.max(synthetic_progress);
    let detail = if elapsed_secs < 10 {
        t!("setup.uv_parsing").to_string()
    } else {
        t!("setup.uv_syncing", secs = elapsed_secs.to_string()).to_string()
    };

    SplashUpdate::loading(t!("setup.installing_deps"), detail, progress)
        .with_subtitle(t!("setup.syncing_deps", tip = get_tip()))
}

fn uv_sync_project(
    status_updater: impl FnMut(SplashUpdate),
    bootstrap_uv: &Path,
    cancel_requested: &AtomicBool,
) -> Result<()> {
    let bootstrap_uv = bootstrap_uv.to_path_buf();
    run_command_with_retry(
        move || {
            let mut cmd = Command::new(&bootstrap_uv);
            cmd.args(["sync", "--frozen", "--no-dev", "--no-install-project"])
                .env("UV_NO_PROGRESS", "1")
                .env("UV_PYTHON_INSTALL_DIR", venv_python_install_dir());
            cmd
        },
        status_updater,
        cancel_requested,
    )
}

fn runtime_tools_update(
    title: impl Into<String>,
    detail: impl Into<String>,
    progress: u8,
) -> SplashUpdate {
    SplashUpdate::loading(title, detail, progress)
        .with_subtitle(t!("setup.rebuilding_env", tip = get_tip()).to_string())
}

fn runtime_wait_update(
    title: &str,
    action: &str,
    elapsed_ticks: u16,
    start_progress: u8,
    end_progress: u8,
) -> SplashUpdate {
    let elapsed_secs = elapsed_ticks / 10;
    let progress = scale_progress(elapsed_secs.min(120) as u8, start_progress, end_progress);
    let detail = if elapsed_secs < 8 {
        t!("setup.action_wait", action = action).to_string()
    } else {
        t!(
            "setup.action_elapsed",
            action = action,
            secs = elapsed_secs.to_string()
        )
        .to_string()
    };
    runtime_tools_update(title, detail, progress)
}

fn ensure_runtime_tools(
    bootstrap_uv: &Path,
    cancel_requested: &AtomicBool,
    mut status_updater: impl FnMut(SplashUpdate),
) -> Result<()> {
    status_updater(runtime_tools_update(
        t!("setup.preparing_env"),
        t!("setup.checking_python"),
        9,
    ));
    ensure_self_contained_python(bootstrap_uv, cancel_requested, &mut status_updater)?;

    status_updater(runtime_tools_update(
        t!("setup.preparing_env"),
        t!("setup.copying_tools"),
        16,
    ));
    copy_file_if_exists(bootstrap_uv, &venv_uv())?;
    Ok(())
}

fn uv_python_env(cmd: &mut Command) {
    cmd.env("UV_NO_PROGRESS", "1")
        .env("UV_PYTHON_INSTALL_DIR", venv_python_install_dir());
    if std::env::var_os("UV_PYTHON_INSTALL_MIRROR").is_none() {
        cmd.env(
            "UV_PYTHON_INSTALL_MIRROR",
            DEFAULT_UV_PYTHON_INSTALL_MIRRORS[0],
        );
    }
}

fn uv_python_install_mirrors() -> Vec<String> {
    if let Some(mirror) = std::env::var_os("UV_PYTHON_INSTALL_MIRROR") {
        return vec![mirror.to_string_lossy().into_owned()];
    }

    DEFAULT_UV_PYTHON_INSTALL_MIRRORS
        .iter()
        .map(|mirror| (*mirror).to_owned())
        .collect()
}

fn ensure_self_contained_python(
    bootstrap_uv: &Path,
    cancel_requested: &AtomicBool,
    mut status_updater: impl FnMut(SplashUpdate),
) -> Result<()> {
    status_updater(runtime_tools_update(
        t!("setup.preparing_env"),
        t!("setup.checking_python_version", version = PYTHON_VERSION),
        10,
    ));
    if venv_python_works() && managed_python_executable().is_some() {
        return Ok(());
    }

    if managed_python_executable().is_none() {
        fs::create_dir_all(venv_python_install_dir()).with_context(|| {
            t!(
                "errors.python_dir_failed",
                error = venv_python_install_dir().display().to_string()
            )
        })?;
        let mirrors = uv_python_install_mirrors();
        let mut downloaded = false;
        for (index, mirror) in mirrors.iter().enumerate() {
            let mirror_label = if index == 0 {
                t!("setup.primary_mirror").to_string()
            } else {
                t!("setup.fallback_mirror").to_string()
            };
            status_updater(runtime_tools_update(
                t!("setup.download_python_title"),
                t!(
                    "setup.downloading_python",
                    version = PYTHON_VERSION,
                    mirror = mirror_label,
                    current = (index + 1).to_string(),
                    total = mirrors.len().to_string()
                ),
                11,
            ));
            let mut cmd = Command::new(bootstrap_uv);
            cmd.args(["python", "install", "--install-dir"])
                .arg(venv_python_install_dir())
                .args([
                    "--no-bin",
                    "--managed-python",
                    "--mirror",
                    mirror,
                    PYTHON_VERSION,
                ]);
            uv_python_env(&mut cmd);
            let mut elapsed_ticks = 0u16;
            let status = run_status_command_with_tick(&mut cmd, cancel_requested, || {
                elapsed_ticks = elapsed_ticks.saturating_add(1);
                if elapsed_ticks == 1 || elapsed_ticks % 10 == 0 {
                    status_updater(runtime_wait_update(
                        &t!("setup.download_python_title"),
                        &t!("setup.downloading_python_action", version = PYTHON_VERSION),
                        elapsed_ticks,
                        11,
                        13,
                    ));
                }
            })?;
            if status.success() {
                downloaded = true;
                break;
            }
            warn!(
                "{}",
                t!(
                    "errors.download_python_failed_mirror",
                    version = PYTHON_VERSION,
                    mirror = mirror
                )
            );
        }
        if !downloaded {
            bail!(t!(
                "errors.python_download_failed",
                version = PYTHON_VERSION
            ));
        }
    }

    let managed_python = managed_python_executable()
        .ok_or_else(|| anyhow!(t!("errors.python_not_found", version = PYTHON_VERSION)))?;
    status_updater(runtime_tools_update(
        t!("setup.creating_venv_title"),
        t!("setup.creating_venv"),
        13,
    ));
    reset_virtualenv_layout()?;
    let mut cmd = Command::new(bootstrap_uv);
    cmd.args(["venv", "--allow-existing", "--relocatable", "--python"])
        .arg(managed_python)
        .arg(venv_dir());
    uv_python_env(&mut cmd);
    let status = run_status_command(&mut cmd, cancel_requested)?;
    if !status.success() {
        bail!(t!("errors.venv_create_failed"));
    }
    Ok(())
}

fn reset_virtualenv_layout() -> Result<()> {
    let venv = venv_dir();
    let entries = if cfg!(windows) {
        vec!["Scripts", "Lib", "Include", "pyvenv.cfg"]
    } else {
        vec!["bin", "lib", "include", "pyvenv.cfg"]
    };

    for entry in entries {
        let path = venv.join(entry);
        if path.exists() {
            remove_runtime_entry_with_retry(&path).with_context(|| {
                t!(
                    "errors.reset_venv_failed",
                    error = path.display().to_string()
                )
            })?;
        }
    }

    Ok(())
}

fn managed_python_executable() -> Option<PathBuf> {
    let install_dir = venv_python_install_dir();
    let entries = fs::read_dir(install_dir).ok()?;
    let prefix = format!("cpython-{PYTHON_VERSION}-");
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(&prefix) {
            continue;
        }
        let candidates = if cfg!(windows) {
            vec![path.join("python.exe")]
        } else {
            vec![
                path.join("bin").join("python3.14"),
                path.join("bin").join("python"),
            ]
        };
        for candidate in candidates {
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

fn venv_python_works() -> bool {
    let python = venv_python();
    if !python.exists() {
        return false;
    }
    Command::new(python)
        .args([
            "-c",
            "import sys; raise SystemExit(0 if sys.version_info[:2] == (3, 14) else 1)",
        ])
        .create_no_window()
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn copy_file_if_exists(from: &Path, to: &Path) -> Result<()> {
    if !from.exists() {
        return Ok(());
    }
    if to.exists() && files_match(from, to).unwrap_or(false) {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).with_context(|| {
            t!(
                "errors.create_dir_failed",
                path = parent.display().to_string()
            )
        })?;
    }
    fs::copy(from, to).with_context(|| {
        t!(
            "errors.copy_file_failed",
            src = from.display().to_string(),
            dest = to.display().to_string()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(to)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(to, permissions)?;
    }
    Ok(())
}

fn files_match(left: &Path, right: &Path) -> Result<bool> {
    let left_metadata = fs::metadata(left).with_context(|| {
        t!(
            "errors.read_info_failed",
            error = left.display().to_string()
        )
    })?;
    let right_metadata = fs::metadata(right).with_context(|| {
        t!(
            "errors.read_info_failed",
            error = right.display().to_string()
        )
    })?;
    if left_metadata.len() != right_metadata.len() {
        return Ok(false);
    }

    let left_bytes = fs::read(left).with_context(|| {
        t!(
            "errors.read_file_failed",
            error = left.display().to_string()
        )
    })?;
    let right_bytes = fs::read(right).with_context(|| {
        t!(
            "errors.read_file_failed",
            error = right.display().to_string()
        )
    })?;
    Ok(left_bytes == right_bytes)
}

fn splash_update_for_dependency_output(
    line: &str,
    seen_packages: &mut HashSet<String>,
) -> Option<SplashUpdate> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let subtitle = t!("setup.syncing_deps", tip = get_tip()).to_string();
    let deps_title = t!("setup.installing_deps");

    if line.starts_with("Resolved ") {
        return Some(SplashUpdate::loading(deps_title, line, 70).with_subtitle(subtitle));
    }

    if line.starts_with("Downloading ") {
        if let Some(pkg) = extract_uv_package_name(line) {
            seen_packages.insert(pkg);
        }
        let progress = uv_download_progress(seen_packages.len());
        return Some(SplashUpdate::loading(deps_title, line, progress).with_subtitle(subtitle));
    }

    if line.starts_with("Prepared ") {
        return Some(SplashUpdate::loading(deps_title, line, 84).with_subtitle(subtitle));
    }

    if line.starts_with("Installed ") {
        return Some(SplashUpdate::loading(deps_title, line, 88).with_subtitle(subtitle));
    }

    if line.starts_with("+ ") || line.starts_with("Audited ") {
        return Some(SplashUpdate::loading(deps_title, line, 90).with_subtitle(subtitle));
    }

    None
}

fn is_uv_progress_line(line: &str) -> bool {
    line.starts_with("Resolved ")
        || line.starts_with("Downloading ")
        || line.starts_with("Downloaded ")
        || line.starts_with("Prepared ")
        || line.starts_with("Installed ")
        || line.starts_with("Audited ")
        || line.starts_with("warning: ")
        || line.starts_with("hint: ")
        || line.starts_with("note: ")
}

fn extract_uv_package_name(line: &str) -> Option<String> {
    let rest = line.strip_prefix("Downloading ")?;
    let name = rest
        .split_once("==")
        .map(|(n, _)| n)
        .or_else(|| rest.split_once(" @ ").map(|(n, _)| n))
        .or_else(|| rest.split_once(" (").map(|(n, _)| n))
        .unwrap_or(rest);
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn uv_download_progress(downloaded: usize) -> u8 {
    (72 + downloaded.min(10) as u8).min(82)
}

fn scale_progress(percentage: u8, start: u8, end: u8) -> u8 {
    let percentage = percentage.min(100) as u16;
    let start = start as u16;
    let end = end as u16;
    (start + ((percentage * (end - start)) / 100)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ntep-launcher-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn workspace_requires_backend_markers() {
        let dir = test_dir("workspace-markers");
        assert!(!backend_workspace_has_required_files(&dir));

        fs::write(dir.join(BACKEND_ENTRYPOINT), "").unwrap();
        fs::write(dir.join(PYPROJECT_FILE), "").unwrap();
        fs::write(dir.join(UV_LOCK_FILE), "").unwrap();
        assert!(backend_workspace_has_required_files(&dir));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn uv_package_name_is_extracted_from_progress_lines() {
        assert_eq!(
            extract_uv_package_name("Downloading numpy==2.4.3 (8.2 MiB)").as_deref(),
            Some("numpy")
        );
        assert_eq!(
            extract_uv_package_name("Downloading rich @ https://example.invalid/rich.whl")
                .as_deref(),
            Some("rich")
        );
    }
}

use std::{
    collections::BTreeSet,
    net::TcpStream,
    process::{Command, ExitStatus},
    thread::sleep,
    time::Duration,
};

use anyhow::{anyhow, Result};
use command_group::{CommandGroup, GroupChild};
use tracing::{info, warn};

use crate::setup::venv_python;
use crate::window_util::CreateNoWindow as _;

pub const WEBUI_PORT: u16 = 9150;
const BACKEND_SCRIPT: &str = "main.py";
const LAUNCHER_PID_ENV: &str = "NTEP_LAUNCHER_PID";

#[derive(Clone, Debug)]
pub struct WebuiLaunchConfig {
    pub port: u16,
}

impl Default for WebuiLaunchConfig {
    fn default() -> Self {
        Self { port: WEBUI_PORT }
    }
}

impl WebuiLaunchConfig {
    fn args(&self) -> [&'static str; 1] {
        [BACKEND_SCRIPT]
    }
}

pub struct ManagedBackend {
    child: Option<GroupChild>,
}

impl ManagedBackend {
    pub fn new(config: &WebuiLaunchConfig) -> Result<Self> {
        std::env::set_var(LAUNCHER_PID_ENV, format!("{}", std::process::id()));
        kill_processes_using_port(config.port)?;

        let child = Command::new(venv_python())
            .args(config.args())
            .group()
            .create_no_window()
            .spawn()?;
        let res = Self { child: Some(child) };

        let address = format!("127.0.0.1:{}", config.port).parse().unwrap();
        let start_time = std::time::Instant::now();
        while start_time.elapsed() < Duration::from_secs(60) {
            if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
                return Ok(res);
            }
            sleep(Duration::from_millis(100));
        }
        Err(anyhow!(
            "Timeout waiting for port {} to be ready",
            config.port
        ))
    }

    pub fn terminate(&mut self) -> Result<ExitStatus> {
        if let Some(mut child) = self.child.take() {
            #[cfg(unix)]
            {
                use command_group::{Signal, UnixChildExt};
                let _ = child.signal(Signal::SIGTERM);
                let start_time = std::time::Instant::now();
                while start_time.elapsed() < Duration::from_millis(500) {
                    if let Ok(Some(exit_status)) = child.try_wait() {
                        return Ok(exit_status);
                    }
                    sleep(Duration::from_millis(100));
                }
                warn!("{BACKEND_SCRIPT} didn't exit, killing it...");
            }
            child.kill()?;
            Ok(child.wait()?)
        } else {
            Ok(ExitStatus::default())
        }
    }
}

fn kill_processes_using_port(port: u16) -> Result<()> {
    let pids = match pids_using_tcp_port(port) {
        Ok(pids) => pids,
        Err(e) => {
            warn!("Unable to scan processes using port {}: {}", port, e);
            return Ok(());
        }
    };
    if pids.is_empty() {
        return Ok(());
    }

    let current_pid = std::process::id();
    let sys = sysinfo::System::new_all();
    for pid in pids {
        if pid == 0 || pid == current_pid {
            continue;
        }

        let sys_pid = sysinfo::Pid::from_u32(pid);
        match sys.process(sys_pid) {
            Some(process) => {
                info!(
                    "Killing process {} ({}) using configured WebUI port {}",
                    pid,
                    process.name().to_string_lossy(),
                    port
                );
                if !process.kill() {
                    warn!("Failed to kill process {} using port {}", pid, port);
                }
            }
            None => {
                warn!(
                    "Process {} was using port {}, but exited before it could be killed",
                    pid, port
                );
            }
        }
    }

    let start_time = std::time::Instant::now();
    while start_time.elapsed() < Duration::from_secs(5) {
        match pids_using_tcp_port(port) {
            Ok(pids) if pids.is_empty() => return Ok(()),
            Ok(_) => sleep(Duration::from_millis(100)),
            Err(e) => {
                warn!("Unable to verify port {} was released: {}", port, e);
                return Ok(());
            }
        }
    }

    warn!("Timed out waiting for port {} to be released", port);
    Ok(())
}

#[cfg(windows)]
fn pids_using_tcp_port(port: u16) -> Result<BTreeSet<u32>> {
    let output = Command::new("netstat")
        .args(["-ano", "-p", "tcp"])
        .create_no_window()
        .output()?;
    if !output.status.success() {
        return Err(anyhow!("netstat failed with status {}", output.status));
    }

    Ok(parse_windows_netstat_pids(&output.stdout, port))
}

#[cfg(windows)]
fn parse_windows_netstat_pids(output: &[u8], port: u16) -> BTreeSet<u32> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let parts: Vec<_> = line.split_whitespace().collect();
            if parts.len() < 5
                || !parts[0].eq_ignore_ascii_case("TCP")
                || !parts[3].eq_ignore_ascii_case("LISTENING")
                || !local_address_uses_port(parts[1], port)
            {
                return None;
            }
            parts.last()?.parse::<u32>().ok()
        })
        .collect()
}

#[cfg(unix)]
fn pids_using_tcp_port(port: u16) -> Result<BTreeSet<u32>> {
    let output = Command::new("lsof")
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-t"])
        .create_no_window()
        .output()?;
    if !output.status.success() && output.stdout.is_empty() {
        return Ok(BTreeSet::new());
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .collect())
}

#[cfg(windows)]
fn local_address_uses_port(address: &str, port: u16) -> bool {
    address
        .rsplit_once(':')
        .and_then(|(_, port_part)| port_part.parse::<u16>().ok())
        == Some(port)
}

impl Drop for ManagedBackend {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            match child.kill() {
                Ok(_) => {}
                Err(e) => warn!("Failed to kill {BACKEND_SCRIPT} process: {:?}", e),
            }
        }
        // Kill potential leaked processes
        let sys = sysinfo::System::new_all();
        for (pid, process) in sys.processes() {
            for var in process.environ() {
                if pid.as_u32() != std::process::id()
                    && var.to_str().unwrap_or_default()
                        == format!("{LAUNCHER_PID_ENV}={}", std::process::id())
                {
                    process.kill();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_backend_uses_ntep_port() {
        assert_eq!(WEBUI_PORT, 9150);
        assert_eq!(WebuiLaunchConfig::default().port, 9150);
    }

    #[test]
    fn backend_args_only_launch_main_py() {
        assert_eq!(WebuiLaunchConfig::default().args(), ["main.py"]);
    }
}

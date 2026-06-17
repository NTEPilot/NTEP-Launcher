fn main() {
    let sys = sysinfo::System::new_all();
    for (pid, process) in sys.processes() {
        for var in process.environ() {
            if var
                .to_str()
                .unwrap_or_default()
                .starts_with("NTEP_LAUNCHER_PID")
            {
                println!("[{pid}] {var:?}");
            }
        }
    }
}

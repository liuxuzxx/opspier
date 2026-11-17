fn main() {
    #[cfg(target_os = "linux")]
    {
        let args: Vec<_> = std::env::args_os().collect();
        if args
            .get(1)
            .is_some_and(|v| v == "--opspier-terminal-supervisor")
        {
            if args.len() != 4 {
                std::process::exit(2);
            }
            std::process::exit(opspier::kubectl_terminal::supervisor::run(
                std::path::Path::new(&args[2]),
                std::path::Path::new(&args[3]),
            ));
        }
    }
    opspier::run()
}

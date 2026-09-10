fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let directory = args
        .next()
        .ok_or("one empty private temporary directory is required")?;
    if args.next().is_some() {
        return Err("only the disposable directory is accepted".into());
    }
    retract_lib::run_discord_import_benchmark(std::path::Path::new(&directory))
}

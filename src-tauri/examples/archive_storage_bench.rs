fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let directory = args
        .next()
        .ok_or("one exclusively owned empty disposable directory is required")?;
    if args.next().is_some() {
        return Err("only the disposable directory is accepted".into());
    }
    retract_lib::run_archive_storage_benchmark(std::path::Path::new(&directory))
}

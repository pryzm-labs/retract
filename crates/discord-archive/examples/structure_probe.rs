//! Private, opt-in helper. Run only on an explicitly authorized selected input.
use discord_archive::{ArchiveInventory, ArchiveLimits, Cancellation, StructureProbe};
use std::{
    fs::{self, File},
    process::ExitCode,
};

struct NeverCancel;
impl Cancellation for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

fn run() -> Result<(), &'static str> {
    let mut arguments = std::env::args_os().skip(1);
    let input = arguments.next().ok_or("expected one input file")?;
    if input == "-" || arguments.next().is_some() {
        return Err("expected one input file");
    }
    let before = fs::symlink_metadata(&input).map_err(|_| "input unavailable")?;
    if !before.file_type().is_file() {
        return Err("expected a regular input file");
    }
    let file = File::open(&input).map_err(|_| "input unavailable")?;
    let opened = file.metadata().map_err(|_| "input unavailable")?;
    let after = fs::symlink_metadata(&input).map_err(|_| "input unavailable")?;
    if !opened.is_file()
        || !after.file_type().is_file()
        || before.len() != opened.len()
        || opened.len() != after.len()
    {
        return Err("input changed");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != opened.dev()
            || before.ino() != opened.ino()
            || after.dev() != opened.dev()
            || after.ino() != opened.ino()
        {
            return Err("input changed");
        }
    }
    let mut archive = ArchiveInventory::inspect(file, ArchiveLimits::default(), &NeverCancel)
        .map_err(|_| "archive rejected")?;
    let selection = archive.json_entries();
    let report =
        StructureProbe::inspect(&mut archive, &selection).map_err(|_| "structure rejected")?;
    serde_json::to_writer(std::io::stdout().lock(), &report).map_err(|_| "report output failed")?;
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

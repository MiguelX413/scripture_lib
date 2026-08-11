use std::env;
use std::process::ExitCode;

use scripture_lib::ScriptureLibrary;

fn main() -> ExitCode {
    let root = env::args_os().nth(1).unwrap_or_else(|| ".".into());
    match ScriptureLibrary::discover(root) {
        Ok(library) => {
            for bundle in library.bundles() {
                println!(
                    "{}\t{}\t{}\t{} books",
                    bundle.abbreviation,
                    bundle.locale,
                    bundle.name,
                    bundle.books().len()
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

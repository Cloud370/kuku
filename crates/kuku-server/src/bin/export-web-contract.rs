use std::path::PathBuf;

fn main() {
    let output_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            eprintln!("usage: export-web-contract <output-directory>");
            std::process::exit(2);
        });

    if let Err(error) = kuku_server::api::contract_export::export_to(&output_dir) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

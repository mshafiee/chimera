fn main() {
    if let Err(e) = chimera_operator::tools::import_keypair::main() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

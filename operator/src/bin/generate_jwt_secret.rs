fn main() {
    if let Err(e) = chimera_operator::tools::generate_jwt_secret::main() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

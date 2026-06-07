use std::{env, error::Error};

use neo_ai::ModelRegistry;

fn main() -> Result<(), Box<dyn Error>> {
    let Some(path) = env::args().nth(1) else {
        eprintln!("usage: model_catalog <catalog.json>");
        std::process::exit(2);
    };

    let mut registry = ModelRegistry::seeded();
    registry.load_catalog_path(&path)?;

    for model in registry.list() {
        println!("{}/{} {:?}", model.provider.0, model.model, model.api);
    }

    Ok(())
}

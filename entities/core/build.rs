use walkdir::WalkDir;

fn main() {
    let inputs: Vec<_> = WalkDir::new("src/proto")
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path().is_file() && e.path().extension().and_then(|ex| ex.to_str()) == Some("proto")
        })
        .map(|e| e.path().to_string_lossy().to_string())
        .collect();

    protobuf_codegen::Codegen::new()
        .include("src/proto")
        .inputs(inputs)
        .cargo_out_dir("proto")
        .run_from_script();
}

fn main() {
    protobuf_codegen::Codegen::new()
        .include("src/proto")
        .input("src/proto/credentials.proto")
        .cargo_out_dir("proto")
        .run_from_script();
}

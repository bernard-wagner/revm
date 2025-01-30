#[derive(Eq, Hash, PartialEq)]
pub enum WasmTarget {
    WAVM,
    ARM64,
    AMD64,
    HOST,
}

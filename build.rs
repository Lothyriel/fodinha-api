fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR should be set by Cargo");
    power_lua_api::generate::write_files(&out_dir).expect("generate Lua API definitions");
    println!("cargo:rerun-if-changed=crates/power-lua-api");
}

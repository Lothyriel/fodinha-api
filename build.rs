fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR should be set by Cargo");
    let definitions = fodinha_core::models::game::power_lua::lua_codegen::render_definitions();
    std::fs::write(
        std::path::Path::new(&out_dir).join("fodinha.d.lua"),
        definitions,
    )
    .expect("write Lua API definitions");
    println!("cargo:rerun-if-changed=crates/core/src/models/game/power_lua");
    println!("cargo:rerun-if-changed=crates/lua-api-derive");
}

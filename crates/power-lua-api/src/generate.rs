use std::{fmt::Write as _, fs, io, path::Path};

use crate::metadata::{
    CARD_RANK_VALUES, CARD_SUIT_VALUES, EVENT_DEFINITIONS, GAME_TYPE, LuaFieldDefinition,
    LuaTypeDefinition, PASSIVE_HANDLERS, POWER_CARD_TYPE_VALUES, TYPE_DEFINITIONS,
};

pub fn write_files(out_dir: impl AsRef<Path>) -> io::Result<()> {
    let out_dir = out_dir.as_ref();
    fs::create_dir_all(out_dir)?;
    fs::write(out_dir.join("fodinha.d.lua"), render_definitions())?;
    fs::write(
        out_dir.join("power-card-template.lua"),
        render_power_card_template(),
    )?;
    fs::write(
        out_dir.join("mercenary-passive-template.lua"),
        render_mercenary_passive_template(),
    )?;
    Ok(())
}

pub fn render_definitions() -> String {
    let mut out = String::new();

    writeln!(out, "---@meta").unwrap();
    writeln!(out).unwrap();

    writeln!(out, "---@alias PlayerId string").unwrap();
    writeln!(out).unwrap();

    write_string_alias(&mut out, "CardRank", CARD_RANK_VALUES);
    write_string_alias(&mut out, "CardSuit", CARD_SUIT_VALUES);
    write_string_alias(&mut out, "PowerCardType", POWER_CARD_TYPE_VALUES);

    for definition in TYPE_DEFINITIONS {
        if definition.name == GAME_TYPE.name {
            continue;
        }

        write_class(&mut out, definition);
    }

    write_game_class(&mut out);
    write_script_shapes(&mut out);
    write_event_classes(&mut out);
    write_globals(&mut out);

    out
}

pub fn render_power_card_template() -> &'static str {
    r#"---@type PowerCardScript
return {
    type = PowerCardType.Instant,
    mana_cost = 1,
    quantity = 1,
    effect = function(game, card)
    end,
}
"#
}

pub fn render_mercenary_passive_template() -> &'static str {
    r#"---@type MercenaryPassiveScript
return {
    base_life = 50,
    initial_mana = 2,
    on_match_started = function(game, event, mercenary)
    end,
    on_round_start = function(game, event, mercenary)
    end,
}
"#
}

fn write_string_alias(out: &mut String, name: &str, values: &[&str]) {
    writeln!(out, "---@alias {name}").unwrap();
    for value in values {
        writeln!(out, "---| \"{value}\"").unwrap();
    }
    writeln!(out).unwrap();
}

fn write_class(out: &mut String, definition: &LuaTypeDefinition) {
    writeln!(out, "---{}", definition.description).unwrap();
    writeln!(out, "---@class {}", definition.name).unwrap();
    for field in definition.fields {
        write_field(out, field);
    }

    // LuaLS does not consistently connect methods declared on a local table
    // (`function PowerCard:add_mana_cost(...)`) to parameters typed as the
    // `PowerCard` class. Representing the method as a function-valued class
    // field makes completion work for inline script callbacks as well.
    if definition.name == "PowerCard" {
        for method in definition.methods {
            write_method_field(out, definition.name, method);
        }
    }
    writeln!(out).unwrap();

    if !definition.methods.is_empty() && definition.name != "PowerCard" {
        writeln!(out, "local {} = {{}}", definition.name).unwrap();
        writeln!(out).unwrap();

        for method in definition.methods {
            write_method(out, definition.name, method);
        }
    }
}

fn write_method_field(
    out: &mut String,
    class_name: &str,
    method: &crate::metadata::LuaMethodDefinition,
) {
    let params = std::iter::once(format!("self: {class_name}"))
        .chain(
            method
                .params
                .iter()
                .map(|param| format!("{}: {}", param.name, param.lua_type)),
        )
        .collect::<Vec<_>>()
        .join(", ");
    let returns = method.returns.join(", ");

    writeln!(
        out,
        "---@field {} fun({}): {} # {}",
        method.name, params, returns, method.description
    )
    .unwrap();
}

fn write_field(out: &mut String, field: &LuaFieldDefinition) {
    if field.description.is_empty() {
        writeln!(out, "---@field {} {}", field.name, field.lua_type).unwrap();
    } else {
        writeln!(
            out,
            "---@field {} {} # {}",
            field.name, field.lua_type, field.description
        )
        .unwrap();
    }
}

fn write_game_class(out: &mut String) {
    writeln!(out, "---{}", GAME_TYPE.description).unwrap();
    writeln!(out, "---@class Game").unwrap();
    writeln!(out, "local Game = {{}}").unwrap();
    writeln!(out).unwrap();

    for method in GAME_TYPE.methods {
        write_method(out, GAME_TYPE.name, method);
    }
}

fn write_method(out: &mut String, class_name: &str, method: &crate::metadata::LuaMethodDefinition) {
    writeln!(out, "---{}", method.description).unwrap();
    for param in method.params {
        writeln!(out, "---@param {} {}", param.name, param.lua_type).unwrap();
    }
    for lua_return in method.returns {
        writeln!(out, "---@return {lua_return}").unwrap();
    }

    let params = method
        .params
        .iter()
        .map(|param| param.name)
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(out, "function {class_name}:{}({params}) end", method.name).unwrap();
    writeln!(out).unwrap();
}

fn write_script_shapes(out: &mut String) {
    writeln!(out, "---@class PowerCardScript").unwrap();
    writeln!(out, "---@field type PowerCardType").unwrap();
    writeln!(out, "---@field mana_cost integer").unwrap();
    writeln!(out, "---@field quantity integer").unwrap();
    writeln!(out, "---@field effect fun(game: Game, card: PowerCard)").unwrap();
    writeln!(out).unwrap();

    writeln!(out, "---@class MercenaryPassiveScript").unwrap();
    writeln!(out, "---@field base_life integer").unwrap();
    writeln!(out, "---@field initial_mana integer").unwrap();
    for handler in PASSIVE_HANDLERS {
        writeln!(out, "---{}", handler.description).unwrap();
        writeln!(
            out,
            "---@field {}? fun(game: Game, event: {}, mercenary: Mercenary)",
            handler.name, handler.event_class
        )
        .unwrap();
    }
    writeln!(out).unwrap();
}

fn write_event_classes(out: &mut String) {
    for event in EVENT_DEFINITIONS {
        writeln!(out, "---{}", event.description).unwrap();
        writeln!(out, "---@class {}", event.name).unwrap();
        for field in event.fields {
            write_field(out, field);
        }
        writeln!(out).unwrap();
    }

    writeln!(out, "---@alias PassiveGameEvent").unwrap();
    for event in EVENT_DEFINITIONS {
        writeln!(out, "---| {}", event.name).unwrap();
    }
    writeln!(out).unwrap();
}

fn write_globals(out: &mut String) {
    writeln!(out, "---@class PowerCardTypeEnum").unwrap();
    writeln!(out, "---@field Instant PowerCardType").unwrap();
    writeln!(out, "---@field Targetable PowerCardType").unwrap();
    writeln!(out, "---@field Interactive PowerCardType").unwrap();
    writeln!(out, "---@type PowerCardTypeEnum").unwrap();
    writeln!(out, "PowerCardType = {{").unwrap();
    writeln!(out, "    Instant = \"instant\",").unwrap();
    writeln!(out, "    Targetable = \"targetable\",").unwrap();
    writeln!(out, "    Interactive = \"interactive\",").unwrap();
    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();

    writeln!(out, "---@type Game").unwrap();
    writeln!(out, "game = nil").unwrap();
    writeln!(out).unwrap();

    writeln!(out, "---@type PowerCard").unwrap();
    writeln!(out, "card = nil").unwrap();
    writeln!(out).unwrap();

    writeln!(out, "---@type Mercenary").unwrap();
    writeln!(out, "mercenary = nil").unwrap();
    writeln!(out).unwrap();

    writeln!(out, "---@type PassiveGameEvent").unwrap();
    writeln!(out, "event = nil").unwrap();
}

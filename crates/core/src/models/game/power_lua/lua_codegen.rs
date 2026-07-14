use std::{collections::HashSet, fmt::Write as _, sync::OnceLock};

use mlua_extras::typed::{Func, Index, Param, Type};

pub struct LuaTypeRegistration {
    pub name: &'static str,
    pub type_definition: fn() -> Type,
}

pub struct LuaFieldDefinition {
    pub name: &'static str,
    pub lua_type: &'static str,
    pub description: &'static str,
}

pub struct LuaEventDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub fields: &'static [LuaFieldDefinition],
}

pub struct LuaEventRegistration {
    pub definitions: &'static [LuaEventDefinition],
}

pub struct LuaScriptFieldDefinition {
    pub name: &'static str,
    pub lua_type: &'static str,
    pub description: &'static str,
    pub optional: bool,
}

pub struct LuaScriptDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub fields: &'static [LuaScriptFieldDefinition],
}

pub struct LuaScriptRegistration {
    pub definition: LuaScriptDefinition,
}

pub struct LuaEnumVariantDefinition {
    pub name: &'static str,
    pub value: &'static str,
}

pub struct LuaEnumRegistration {
    pub name: &'static str,
    pub type_name: &'static str,
    pub variants: &'static [LuaEnumVariantDefinition],
}

pub struct LuaScalarRegistration {
    pub name: &'static str,
    pub lua_type: &'static str,
}

#[linkme::distributed_slice]
pub static LUA_TYPES: [LuaTypeRegistration] = [..];
#[linkme::distributed_slice]
pub static LUA_EVENTS: [LuaEventRegistration] = [..];
#[linkme::distributed_slice]
pub static LUA_SCRIPTS: [LuaScriptRegistration] = [..];
#[linkme::distributed_slice]
pub static LUA_ENUMS: [LuaEnumRegistration] = [..];
#[linkme::distributed_slice]
pub static LUA_SCALARS: [LuaScalarRegistration] = [..];

pub(crate) fn enum_definition(name: &str) -> &'static LuaEnumRegistration {
    LUA_ENUMS
        .iter()
        .find(|definition| definition.name == name)
        .unwrap_or_else(|| panic!("Lua enum {name} is not registered"))
}

pub fn render_definitions() -> String {
    let mut output = String::from("---@meta\n\n");
    output.push_str(&render_scalars());
    output.push_str(&render_enums());
    output.push_str(&render_types());
    output.push_str(&render_events());
    output.push_str(&render_scripts());
    output
}

fn render_types() -> String {
    let mut output = String::new();
    let mut registrations = LUA_TYPES.iter().collect::<Vec<_>>();
    registrations.sort_unstable_by_key(|registration| registration.name);

    for registration in registrations {
        let Type::Class(class) = (registration.type_definition)() else {
            continue;
        };

        if let Some(doc) = &class.type_doc {
            for line in doc.lines() {
                writeln!(output, "---{line}").unwrap();
            }
        }
        writeln!(output, "---@class {}", registration.name).unwrap();

        let method_names = class.methods.keys().map(index_name).collect::<HashSet<_>>();

        for (name, field) in &class.fields {
            let name = index_name(name);
            if method_names.contains(&name) || is_method_name(registration.name, &name) {
                if let Type::Function { params, returns } = &field.ty {
                    render_function_method(&mut output, registration.name, &name, params, returns);
                }
                continue;
            }
            if let Some(doc) = &field.doc {
                for line in doc.lines() {
                    writeln!(output, "---{line}").unwrap();
                }
            }
            writeln!(
                output,
                "---@field {} {}",
                name,
                type_signature(&field.ty, registration.name, &name, None)
            )
            .unwrap();
        }

        for (name, method) in &class.methods {
            render_method(&mut output, registration.name, &index_name(name), method);
        }

        output.push('\n');
    }

    output
}

fn render_method(output: &mut String, class_name: &str, method_name: &str, method: &Func) {
    if let Some(doc) = &method.doc {
        for line in doc.lines() {
            writeln!(output, "---{line}").unwrap();
        }
    }

    let mut params = vec![format!("self: {class_name}")];
    for (index, param) in method.params.iter().enumerate() {
        let name = param_name(param, index);
        let ty = type_signature(&param.ty, class_name, method_name, Some(&name));
        params.push(format!("{name}: {ty}"));
    }

    let returns = if method.returns.is_empty() {
        String::new()
    } else {
        let values = method
            .returns
            .iter()
            .map(|value| type_signature(&value.ty, class_name, method_name, None))
            .collect::<Vec<_>>()
            .join(", ");
        format!(": {values}")
    };

    writeln!(
        output,
        "---@field {} fun({}){}",
        method_name,
        params.join(", "),
        returns
    )
    .unwrap();
}

fn render_function_method(
    output: &mut String,
    class_name: &str,
    method_name: &str,
    params: &[Param],
    returns: &[mlua_extras::typed::Return],
) {
    let names = method_parameter_names(class_name, method_name);
    let mut rendered_params = vec![format!("self: {class_name}")];
    for (index, param) in params.iter().enumerate() {
        let name = names
            .get(index)
            .map(|name| (*name).to_string())
            .unwrap_or_else(|| param_name(param, index));
        let ty = type_signature(&param.ty, class_name, method_name, Some(&name));
        rendered_params.push(format!("{name}: {ty}"));
    }
    let returns = render_returns(class_name, method_name, returns);
    writeln!(
        output,
        "---@field {} fun({}){}",
        method_name,
        rendered_params.join(", "),
        returns
    )
    .unwrap();
}

fn render_returns(
    class_name: &str,
    method_name: &str,
    returns: &[mlua_extras::typed::Return],
) -> String {
    if returns.is_empty() {
        return String::new();
    }
    let values = returns
        .iter()
        .map(|value| type_signature(&value.ty, class_name, method_name, None))
        .collect::<Vec<_>>()
        .join(", ");
    format!(": {values}")
}

fn is_method_name(class_name: &str, method_name: &str) -> bool {
    !method_parameter_names(class_name, method_name).is_empty()
        || matches!(
            (class_name, method_name),
            ("Game", "get_current_trump") | ("Game", "player_ids")
        )
}

fn method_parameter_names(class_name: &str, method_name: &str) -> &'static [&'static str] {
    match (class_name, method_name) {
        ("Game", "get_lives")
        | ("Game", "get_bid")
        | ("Game", "get_rounds")
        | ("Game", "get_mana")
        | ("Game", "get_max_mana")
        | ("Game", "get_mana_pool")
        | ("Game", "get_cards")
        | ("Game", "get_power_cards") => &["player_id"],
        ("Game", "add_lives") | ("Game", "add_mana") => &["player_id", "delta"],
        ("Game", "set_lives") => &["player_id", "lifes"],
        ("Game", "add_bids") => &["player_id", "bid_count"],
        ("Game", "set_mana") | ("Game", "set_max_mana") => &["player_id", "mana"],
        ("Game", "switch_cards") => &[
            "first_player_id",
            "first_card",
            "second_player_id",
            "second_card",
        ],
        ("Game", "reveal_deck") => &["caster_id", "target_player_id"],
        ("Game", "steal_power_card") => &["from_player_id", "card_id", "to_player_id"],
        ("Game", "draw_power_cards") => &["player_id", "count"],
        ("PowerCard", "add_mana_cost") => &["delta"],
        _ => &[],
    }
}

fn param_name(param: &Param, index: usize) -> String {
    param
        .name
        .as_deref()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("param{}", index + 1))
}

fn index_name(index: &Index) -> String {
    index.to_string()
}

fn type_signature(
    ty: &Type,
    class_name: &str,
    member_name: &str,
    parameter_name: Option<&str>,
) -> String {
    let mut signature = match ty {
        Type::Single(value) => public_type_name(value),
        Type::Value(inner) | Type::Alias(inner) => {
            type_signature(inner, class_name, member_name, parameter_name)
        }
        Type::Array(inner) => format!(
            "{}[]",
            type_signature(inner, class_name, member_name, parameter_name)
        ),
        Type::Map(key, value) => format!(
            "table<{}, {}>",
            type_signature(key, class_name, member_name, parameter_name),
            type_signature(value, class_name, member_name, parameter_name)
        ),
        Type::Union(types) => types
            .iter()
            .map(|value| type_signature(value, class_name, member_name, parameter_name))
            .collect::<Vec<_>>()
            .join(" | "),
        Type::Tuple(types) => format!(
            "[{}]",
            types
                .iter()
                .map(|value| type_signature(value, class_name, member_name, parameter_name))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Type::Function { params, returns } => {
            let params = params
                .iter()
                .enumerate()
                .map(|(index, param)| {
                    format!(
                        "{}: {}",
                        param_name(param, index),
                        type_signature(&param.ty, class_name, member_name, None)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let returns = if returns.is_empty() {
                String::new()
            } else {
                format!(
                    ": {}",
                    returns
                        .iter()
                        .map(|value| type_signature(&value.ty, class_name, member_name, None))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            format!("fun({params}){returns}")
        }
        Type::Table(entries) => entries
            .iter()
            .map(|(key, value)| {
                format!(
                    "{}: {}",
                    key,
                    type_signature(value, class_name, member_name, None)
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
        Type::Class(_) | Type::Enum(_) => "any".to_string(),
    };

    if class_name == "Card" && member_name == "rank" {
        signature = "Rank".to_string();
    } else if class_name == "Card" && member_name == "suit" {
        signature = "Suit".to_string();
    } else if class_name == "PowerCardState" && member_name == "type" {
        signature = "PowerCardType".to_string();
    } else if class_name == "Game" && member_name == "get_current_trump" && parameter_name.is_none()
    {
        signature = "Rank".to_string();
    } else if class_name == "Game" && member_name == "player_ids" && parameter_name.is_none() {
        signature = "PlayerId[]".to_string();
    } else if parameter_name.is_some_and(is_player_id_name) {
        signature = "PlayerId".to_string();
    } else if parameter_name.is_none() && is_player_id_name(member_name) {
        signature = signature.replace("string", "PlayerId");
    }

    signature
}

fn public_type_name(value: &str) -> String {
    LUA_TYPES
        .iter()
        .find(|registration| {
            value == registration.name
                || value
                    .strip_prefix("Lua")
                    .is_some_and(|name| name == registration.name)
        })
        .map(|registration| registration.name.to_string())
        .unwrap_or_else(|| value.to_string())
}

fn is_player_id_name(name: &str) -> bool {
    matches!(
        name,
        "caster_id"
            | "player_id"
            | "owner_id"
            | "targets"
            | "target_player_id"
            | "first_player_id"
            | "second_player_id"
            | "from_player_id"
            | "to_player_id"
            | "winner"
    )
}

fn render_enums() -> String {
    let mut output = String::new();
    let mut enums = LUA_ENUMS.iter().collect::<Vec<_>>();
    enums.sort_unstable_by_key(|definition| definition.name);
    for definition in enums {
        output.push_str(&format!(
            "---@enum {}\n{} = {{\n",
            definition.name, definition.name
        ));
        for variant in definition.variants {
            output.push_str(&format!("    {} = \"{}\",\n", variant.name, variant.value));
        }
        output.push_str("}\n\n");
    }
    output
}

fn render_scalars() -> String {
    let mut output = String::new();
    let mut scalars = LUA_SCALARS.iter().collect::<Vec<_>>();
    scalars.sort_unstable_by_key(|scalar| scalar.name);
    for scalar in scalars {
        output.push_str(&format!(
            "---@alias {} {}\n\n",
            scalar.name, scalar.lua_type
        ));
    }
    output
}

fn render_scripts() -> String {
    let mut output = String::new();
    let mut scripts = LUA_SCRIPTS.iter().collect::<Vec<_>>();
    scripts.sort_unstable_by_key(|registration| registration.definition.name);
    for registration in scripts {
        let definition = &registration.definition;
        if !definition.description.is_empty() {
            output.push_str(&format!("---{}\n", definition.description));
        }
        output.push_str(&format!("---@class {}\n", definition.name));
        for field in definition.fields {
            let field_name = if field.optional {
                format!("{}?", field.name)
            } else {
                field.name.to_string()
            };
            if field.description.is_empty() {
                output.push_str(&format!("---@field {} {}\n", field_name, field.lua_type));
            } else {
                output.push_str(&format!(
                    "---@field {} {} # {}\n",
                    field_name, field.lua_type, field.description
                ));
            }
        }
        output.push('\n');
    }
    output
}

pub fn render_power_card_template() -> &'static str {
    static TEMPLATE: OnceLock<String> = OnceLock::new();
    TEMPLATE
        .get_or_init(|| {
            let definition = script_definition("PowerCardScript");
            let fields = definition.fields;
            let mut output = String::new();
            writeln!(output, "---@type PowerCardScript").unwrap();
            writeln!(output, "return {{").unwrap();
            writeln!(output, "    {} = PowerCardType.Instant,", fields[0].name).unwrap();
            writeln!(output, "    {} = 1,", fields[1].name).unwrap();
            writeln!(output, "    {} = 1,", fields[2].name).unwrap();
            writeln!(output, "    {} = function(game, card)", fields[3].name).unwrap();
            writeln!(output, "    end,").unwrap();
            writeln!(output, "}}\n").unwrap();
            output
        })
        .as_str()
}

pub fn render_mercenary_passive_template() -> &'static str {
    static TEMPLATE: OnceLock<String> = OnceLock::new();
    TEMPLATE
        .get_or_init(|| {
            let definition = script_definition("MercenaryPassiveScript");
            let mut output = String::new();
            writeln!(output, "---@type MercenaryPassiveScript").unwrap();
            writeln!(output, "return {{").unwrap();
            writeln!(output, "    {} = 50,", definition.fields[0].name).unwrap();
            writeln!(output, "    {} = 2,", definition.fields[1].name).unwrap();
            for field in &definition.fields[2..] {
                writeln!(
                    output,
                    "    {} = function(game, event, mercenary)",
                    field.name
                )
                .unwrap();
                writeln!(output, "    end,").unwrap();
            }
            writeln!(output, "}}\n").unwrap();
            output
        })
        .as_str()
}

fn script_definition(name: &str) -> &'static LuaScriptDefinition {
    LUA_SCRIPTS
        .iter()
        .find(|registration| registration.definition.name == name)
        .map(|registration| &registration.definition)
        .unwrap_or_else(|| panic!("Lua script definition {name} is not registered"))
}

pub(crate) fn passive_handler_name(event_type: &str) -> &'static str {
    script_definition("MercenaryPassiveScript")
        .fields
        .iter()
        .find(|field| field.name.strip_prefix("on_") == Some(event_type))
        .map(|field| field.name)
        .unwrap_or_else(|| panic!("no passive handler registered for {event_type}"))
}

pub(crate) fn is_passive_handler(name: &str) -> bool {
    script_definition("MercenaryPassiveScript")
        .fields
        .iter()
        .any(|field| field.name == name)
}

fn render_events() -> String {
    let mut output = String::new();
    let mut events = LUA_EVENTS
        .iter()
        .flat_map(|registration| registration.definitions.iter())
        .collect::<Vec<_>>();
    events.sort_unstable_by_key(|event| event.name);
    for event in &events {
        if !event.description.is_empty() {
            output.push_str(&format!("---{}\n", event.description));
        }
        output.push_str(&format!("---@class {}\n", event.name));
        for field in event.fields {
            if field.description.is_empty() {
                output.push_str(&format!("---@field {} {}\n", field.name, field.lua_type));
            } else {
                output.push_str(&format!(
                    "---@field {} {} # {}\n",
                    field.name, field.lua_type, field.description
                ));
            }
        }
        output.push('\n');
    }
    output.push_str("---@alias PassiveGameEvent\n");
    for event in events {
        output.push_str(&format!("---| {}\n", event.name));
    }
    output.push('\n');
    output
}

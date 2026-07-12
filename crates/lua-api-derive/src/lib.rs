use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Data, DeriveInput, LitStr, Token, parse_macro_input, parse_quote, punctuated::Punctuated,
    spanned::Spanned,
};

#[proc_macro_derive(LuaApiType, attributes(lua_api_type))]
pub fn derive_lua_api_type(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ident = input.ident;
    let name = attribute_string_for(&input.attrs, "lua_api_type", "name").unwrap_or_else(|| {
        LitStr::new(
            lua_type_name_default(&ident.to_string()).as_str(),
            ident.span(),
        )
    });

    let registry: syn::Path =
        parse_quote!(crate::models::game::power_lua::lua_codegen::LuaTypeRegistration);
    let registration = syn::Ident::new(&format!("__LUA_TYPE_REGISTRATION_{ident}"), ident.span());

    quote! {
        #[allow(non_upper_case_globals)]
        #[linkme::distributed_slice(crate::models::game::power_lua::lua_codegen::LUA_TYPES)]
        static #registration: #registry = #registry {
                name: #name,
                type_definition: <#ident as mlua_extras::typed::Typed>::ty,
        };
    }
    .into()
}

fn lua_type_name_default(name: &str) -> String {
    name.strip_prefix("Lua").unwrap_or(name).to_string()
}

#[proc_macro_derive(LuaApiEnum, attributes(lua_api_enum))]
pub fn derive_lua_api_enum(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ident = input.ident;
    let name = attribute_string_for(&input.attrs, "lua_api_enum", "name")
        .unwrap_or_else(|| LitStr::new(&ident.to_string(), ident.span()));
    let type_name = attribute_string_for(&input.attrs, "lua_api_enum", "type_name")
        .unwrap_or_else(|| LitStr::new(&ident.to_string(), ident.span()));
    let rename_all = attribute_string_for(&input.attrs, "serde", "rename_all");
    let registry: syn::Path =
        parse_quote!(crate::models::game::power_lua::lua_codegen::LuaEnumRegistration);
    let registration = syn::Ident::new(&format!("__LUA_ENUM_REGISTRATION_{ident}"), ident.span());
    let variants = match &input.data {
        Data::Enum(data) => data.variants.iter().map(|variant| {
            let variant_name = variant.ident.to_string();
            let value = attribute_string_for(&variant.attrs, "lua_api_enum", "value")
                .unwrap_or_else(|| {
                    let value = match rename_all.as_ref().map(LitStr::value).as_deref() {
                        Some("snake_case") => snake_case(&variant_name),
                        _ => variant_name.clone(),
                    };
                    LitStr::new(&value, variant.ident.span())
                });
            let name = LitStr::new(&variant_name, variant.ident.span());
            quote! {
                crate::models::game::power_lua::lua_codegen::LuaEnumVariantDefinition { name: #name, value: #value }
            }
        }).collect::<Vec<_>>(),
        _ => return syn::Error::new_spanned(ident, "LuaApiEnum requires an enum").to_compile_error().into(),
    };
    let into_lua_arms = match &input.data {
        Data::Enum(data) => data
            .variants
            .iter()
            .map(|variant| {
                let variant_name = variant.ident.to_string();
                let value = attribute_string_for(&variant.attrs, "lua_api_enum", "value")
                    .unwrap_or_else(|| {
                        let value = match rename_all.as_ref().map(LitStr::value).as_deref() {
                            Some("snake_case") => snake_case(&variant_name),
                            _ => variant_name.clone(),
                        };
                        LitStr::new(&value, variant.ident.span())
                    });
                let ident = &variant.ident;
                quote! { Self::#ident => #value }
            })
            .collect::<Vec<_>>(),
        _ => unreachable!(),
    };
    quote! {
        #[allow(non_upper_case_globals)]
        #[linkme::distributed_slice(crate::models::game::power_lua::lua_codegen::LUA_ENUMS)]
        static #registration: #registry = #registry {
            name: #name,
            type_name: #type_name,
            variants: &[#(#variants),*],
        };

        impl mlua_extras::mlua::IntoLua for #ident {
            fn into_lua(self, lua: &mlua_extras::mlua::Lua) -> mlua_extras::mlua::Result<mlua_extras::mlua::Value> {
                Ok(mlua_extras::mlua::Value::String(lua.create_string(self.lua_name())?))
            }
        }

        impl #ident {
            pub fn lua_name(self) -> &'static str {
                match self {
                    #(#into_lua_arms),*
                }
            }
        }

        impl mlua_extras::typed::Typed for #ident {
            fn ty() -> mlua_extras::typed::Type {
                mlua_extras::typed::Type::named(#type_name)
            }
        }
    }.into()
}

#[proc_macro_derive(LuaApiScalar, attributes(lua_api_scalar))]
pub fn derive_lua_api_scalar(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ident = input.ident;
    let name = LitStr::new(&ident.to_string(), ident.span());
    let lua_type = attribute_string_for(&input.attrs, "lua_api_scalar", "lua_type")
        .or_else(|| scalar_type_from_struct(&input.data))
        .unwrap_or_else(|| LitStr::new("any", ident.span()));
    let registration = syn::Ident::new(&format!("__LUA_SCALAR_REGISTRATION_{ident}"), ident.span());
    quote! {
        #[allow(non_upper_case_globals)]
        #[linkme::distributed_slice(crate::models::game::power_lua::lua_codegen::LUA_SCALARS)]
        static #registration: crate::models::game::power_lua::lua_codegen::LuaScalarRegistration = crate::models::game::power_lua::lua_codegen::LuaScalarRegistration {
            name: #name,
            lua_type: #lua_type,
        };
    }.into()
}

#[proc_macro_derive(LuaApiEvent, attributes(lua_api_event, lua_api_field))]
pub fn derive_lua_api_event(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ident = input.ident;
    let registry: syn::Path =
        parse_quote!(crate::models::game::power_lua::lua_codegen::LuaEventRegistration);
    let registration = syn::Ident::new(&format!("__LUA_EVENT_REGISTRATION_{ident}"), ident.span());
    let data = match input.data {
        Data::Enum(data) => data,
        _ => {
            return syn::Error::new_spanned(ident, "LuaApiEvent requires an enum")
                .to_compile_error()
                .into();
        }
    };

    let events = data.variants.iter().map(|variant| {
        let variant_name = &variant.ident;
        let event_name = attribute_string_for(&variant.attrs, "lua_api_event", "name")
            .unwrap_or_else(|| LitStr::new(&format!("{variant_name}Event"), variant_name.span()));
        let event_type = attribute_string_for(&variant.attrs, "lua_api_event", "type")
            .unwrap_or_else(|| LitStr::new(&snake_case(&variant_name.to_string()), variant_name.span()));
        let description = attribute_string_for(&variant.attrs, "lua_api_event", "description")
            .unwrap_or_else(|| LitStr::new("", variant_name.span()));
        let fields = variant.fields.iter().filter_map(|field| {
            let field_ident = field.ident.as_ref()?;
            let name = field_ident.to_string();
            let lua_name = field_attribute_string(&field.attrs, "name")
                .unwrap_or_else(|| LitStr::new(&name, field_ident.span()));
            let lua_type = script_field_lua_type(field_ident, &field.ty);
            Some(quote! { crate::models::game::power_lua::lua_codegen::LuaFieldDefinition { name: #lua_name, lua_type: #lua_type, description: "" } })
        }).collect::<Vec<_>>();
        let discriminator_type = LitStr::new(&format!("\"{}\"", event_type.value()), event_type.span());
        let discriminator = quote! { crate::models::game::power_lua::lua_codegen::LuaFieldDefinition { name: "type", lua_type: #discriminator_type, description: "Event discriminator." } };
        quote! {
            crate::models::game::power_lua::lua_codegen::LuaEventDefinition { name: #event_name, description: #description, fields: &[#discriminator, #(#fields),*] }
        }
    }).collect::<Vec<_>>();
    let definitions_ident =
        syn::Ident::new(&format!("__LUA_EVENT_DEFINITIONS_{ident}"), ident.span());

    quote! {
        #[allow(non_upper_case_globals)]
        static #definitions_ident: &[crate::models::game::power_lua::lua_codegen::LuaEventDefinition] = &[#(#events),*];
        #[allow(non_upper_case_globals)]
        #[linkme::distributed_slice(crate::models::game::power_lua::lua_codegen::LUA_EVENTS)]
        static #registration: #registry = #registry { definitions: #definitions_ident };
    }.into()
}

#[proc_macro_derive(LuaApiScript, attributes(lua_api_script, description, lua_api_field))]
pub fn derive_lua_api_script(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ident = input.ident;
    let name = attribute_string_for(&input.attrs, "lua_api_script", "name")
        .unwrap_or_else(|| LitStr::new(&ident.to_string(), ident.span()));
    let description = attribute_string_for(&input.attrs, "lua_api_script", "description")
        .unwrap_or_else(|| LitStr::new("", ident.span()));
    let registry: syn::Path =
        parse_quote!(crate::models::game::power_lua::lua_codegen::LuaScriptRegistration);
    let registration = syn::Ident::new(&format!("__LUA_SCRIPT_REGISTRATION_{ident}"), ident.span());
    let fields = match input.data {
        Data::Struct(data) => data
            .fields
            .iter()
            .filter_map(|field| {
                let field_ident = field.ident.as_ref()?;
                let field_name = field_attribute_string(&field.attrs, "name")
                    .unwrap_or_else(|| LitStr::new(&field_ident.to_string(), field_ident.span()));
                let description = field_description(&field.attrs);
                let lua_type = script_field_lua_type(field_ident, &field.ty);
                let optional = field_ident.to_string().starts_with("on_");
                Some(quote! {
                    crate::models::game::power_lua::lua_codegen::LuaScriptFieldDefinition {
                        name: #field_name,
                        lua_type: #lua_type,
                        description: #description,
                        optional: #optional,
                    }
                })
            })
            .collect::<Vec<_>>(),
        _ => {
            return syn::Error::new_spanned(ident, "LuaApiScript requires a struct")
                .to_compile_error()
                .into();
        }
    };
    quote! {
        #[allow(non_upper_case_globals)]
        #[linkme::distributed_slice(crate::models::game::power_lua::lua_codegen::LUA_SCRIPTS)]
        static #registration: #registry = #registry {
            definition: crate::models::game::power_lua::lua_codegen::LuaScriptDefinition {
                name: #name,
                description: #description,
                fields: &[#(#fields),*],
            },
        };
    }
    .into()
}

fn attribute_string_for(
    attrs: &[syn::Attribute],
    attribute_name: &str,
    key: &str,
) -> Option<LitStr> {
    attrs
        .iter()
        .find(|attribute| attribute.path().is_ident(attribute_name))
        .and_then(|attribute| {
            let metas = attribute
                .parse_args_with(Punctuated::<syn::MetaNameValue, Token![,]>::parse_terminated)
                .ok()?;
            let meta = metas.iter().find(|meta| meta.path.is_ident(key))?;
            match &meta.value {
                syn::Expr::Lit(expr) => match &expr.lit {
                    syn::Lit::Str(value) => Some(value.clone()),
                    _ => None,
                },
                _ => None,
            }
        })
}

fn field_attribute_string(attrs: &[syn::Attribute], key: &str) -> Option<LitStr> {
    attribute_string_for(attrs, "lua_api_field", key)
}

fn field_description(attrs: &[syn::Attribute]) -> LitStr {
    attribute_string_for(attrs, "description", "value")
        .or_else(|| {
            attrs
                .iter()
                .find(|attribute| attribute.path().is_ident("description"))
                .and_then(|attribute| attribute.parse_args::<LitStr>().ok())
        })
        .unwrap_or_else(|| LitStr::new("", proc_macro2::Span::call_site()))
}

fn scalar_type_from_struct(data: &Data) -> Option<LitStr> {
    let Data::Struct(data) = data else {
        return None;
    };
    let field = data.fields.iter().next()?;
    Some(lua_type_for(&field.ty))
}

fn script_field_lua_type(field: &syn::Ident, ty: &syn::Type) -> LitStr {
    let field_name = field.to_string();
    if let Some(event) = field_name.strip_prefix("on_") {
        return LitStr::new(
            &format!(
                "fun(game: Game, event: {}Event, mercenary: Mercenary)",
                pascal_case(event)
            ),
            field.span(),
        );
    }
    if field_name == "effect" {
        return LitStr::new("fun(game: Game, card: PowerCard)", field.span());
    }
    lua_type_for(ty)
}

fn lua_type_for(ty: &syn::Type) -> LitStr {
    let text = quote!(#ty).to_string().replace(' ', "");
    let lua = lua_type_name(&text);
    LitStr::new(&lua, ty.span())
}

fn lua_type_name(text: &str) -> String {
    let text = text.trim_start_matches('&');
    if let Some(inner) = text
        .strip_prefix("&[")
        .and_then(|inner| inner.strip_suffix(']'))
    {
        return format!("{}[]", lua_type_name(inner));
    }
    if let Some(inner) = text
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
    {
        let element = inner.split_once(';').map_or(inner, |(element, _)| element);
        return format!("{}[]", lua_type_name(element));
    }
    if let Some(inner) = generic_inner(text, "Option") {
        return format!("{}|nil", lua_type_name(inner));
    }
    if let Some(inner) = generic_inner(text, "Vec") {
        return format!("{}[]", lua_type_name(inner));
    }
    if let Some(inner) = generic_inner(text, "HashMap") {
        let parts = split_generic_args(inner);
        if parts.len() == 2 {
            return format!(
                "table<{}, {}>",
                lua_type_name(parts[0]),
                lua_type_name(parts[1])
            );
        }
    }
    if let Some(inner) = generic_inner(text, "Arc") {
        return lua_type_name(inner);
    }
    let name = text.rsplit("::").next().unwrap_or(text);
    match name {
        "String" | "str" => "string".to_string(),
        "bool" => "boolean".to_string(),
        "f32" | "f64" => "number".to_string(),
        "usize" | "u8" | "u16" | "u32" | "u64" | "u128" | "isize" | "i8" | "i16" | "i32"
        | "i64" | "i128" => "integer".to_string(),
        "()" | "!" => "nil".to_string(),
        _ if is_type_name(name) => name.to_string(),
        _ => "any".to_string(),
    }
}

fn is_type_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_uppercase())
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn generic_inner<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let (head, inner) = text.split_once('<')?;
    if head.rsplit("::").next()? != name {
        return None;
    }
    inner.strip_suffix('>')
}

fn split_generic_args(text: &str) -> Vec<&str> {
    let mut depth = 0;
    let mut start = 0;
    let mut args = Vec::new();
    for (index, character) in text.char_indices() {
        match character {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                args.push(&text[start..index]);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    args.push(&text[start..]);
    args
}

fn snake_case(value: &str) -> String {
    value
        .chars()
        .enumerate()
        .fold(String::new(), |mut out, (i, ch)| {
            if ch.is_uppercase() && i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            out
        })
}

fn pascal_case(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

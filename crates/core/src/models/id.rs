use std::sync::Arc;

use lua_api_derive::LuaApiScalar;

#[derive(Debug, PartialEq, Eq, Hash, Clone, LuaApiScalar)]
pub struct PlayerId(pub Arc<str>);

impl<'de> serde::Deserialize<'de> for PlayerId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(PlayerId(Arc::from(s)))
    }
}

impl serde::Serialize for PlayerId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl AsRef<str> for PlayerId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl PlayerId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl mlua_extras::mlua::FromLua for PlayerId {
    fn from_lua(
        value: mlua_extras::mlua::Value,
        lua: &mlua_extras::mlua::Lua,
    ) -> mlua_extras::mlua::Result<Self> {
        let value = <String as mlua_extras::mlua::FromLua>::from_lua(value, lua)?;
        Ok(Self(Arc::from(value)))
    }
}

impl mlua_extras::mlua::IntoLua for PlayerId {
    fn into_lua(
        self,
        lua: &mlua_extras::mlua::Lua,
    ) -> mlua_extras::mlua::Result<mlua_extras::mlua::Value> {
        Ok(mlua_extras::mlua::Value::String(
            lua.create_string(self.as_str())?,
        ))
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct CardId(pub Arc<str>);

impl serde::Serialize for CardId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for CardId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(CardId(Arc::from(s)))
    }
}

impl CardId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CardId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, serde::Serialize, serde::Deserialize)]
pub struct CardDefinitionRef {
    pub card_id: CardId,
    pub version: i64,
}

impl CardDefinitionRef {
    pub fn new(card_id: CardId, version: i64) -> Self {
        Self { card_id, version }
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct DeckId(pub Arc<str>);

impl serde::Serialize for DeckId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for DeckId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(DeckId(Arc::from(s)))
    }
}

impl DeckId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DeckId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct MercenaryId(pub Arc<str>);

impl serde::Serialize for MercenaryId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for MercenaryId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(MercenaryId(Arc::from(s)))
    }
}

impl MercenaryId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MercenaryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct LobbyId(pub Arc<str>);

pub type MatchId = LobbyId;

impl serde::Serialize for LobbyId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for LobbyId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(LobbyId(Arc::from(s)))
    }
}

impl LobbyId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub struct Uid(pub Arc<str>);

const ALPHABET: &[char] = &[
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I',
    'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '_', 'a',
    'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't',
    'u', 'v', 'w', 'x', 'y', 'z', '-',
];

pub fn gen_playerid() -> PlayerId {
    PlayerId(nanoid::nanoid!(10, ALPHABET).into())
}

pub fn gen_lobbyid() -> LobbyId {
    LobbyId(nanoid::nanoid!(12, ALPHABET).into())
}

pub fn gen_matchid() -> MatchId {
    gen_lobbyid()
}

pub fn gen_cardid() -> CardId {
    CardId(nanoid::nanoid!(16, ALPHABET).into())
}

pub fn gen_deckid() -> DeckId {
    DeckId(nanoid::nanoid!(16, ALPHABET).into())
}

pub fn gen_mercenaryid() -> MercenaryId {
    MercenaryId(nanoid::nanoid!(16, ALPHABET).into())
}

pub fn gen_uid() -> Uid {
    Uid(nanoid::nanoid!(16, ALPHABET).into())
}

#[cfg(test)]
mod tests {
    use super::CardDefinitionRef;

    #[test]
    fn card_definition_ref_serializes_with_version() {
        let card_ref: CardDefinitionRef =
            serde_json::from_str(r#"{"card_id":"card-1","version":2}"#).unwrap();

        assert_eq!(card_ref.card_id.as_str(), "card-1");
        assert_eq!(card_ref.version, 2);
        assert_eq!(
            serde_json::to_value(card_ref).unwrap(),
            serde_json::json!({ "card_id": "card-1", "version": 2 })
        );
    }
}

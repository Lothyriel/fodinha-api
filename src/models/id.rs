use std::sync::Arc;

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
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

pub fn gen_uid() -> Uid {
    Uid(nanoid::nanoid!(16, ALPHABET).into())
}

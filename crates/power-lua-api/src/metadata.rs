#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LuaTypeDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub fields: &'static [LuaFieldDefinition],
    pub methods: &'static [LuaMethodDefinition],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LuaFieldDefinition {
    pub name: &'static str,
    pub lua_type: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LuaMethodDefinition {
    pub name: &'static str,
    pub params: &'static [LuaParameterDefinition],
    pub returns: &'static [&'static str],
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LuaParameterDefinition {
    pub name: &'static str,
    pub lua_type: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LuaEventDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub fields: &'static [LuaFieldDefinition],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LuaPassiveHandlerDefinition {
    pub name: &'static str,
    pub event_type: &'static str,
    pub event_class: &'static str,
    pub description: &'static str,
}

pub const GET_LIVES: &str = "get_lives";
pub const ADD_LIVES: &str = "add_lives";
pub const SET_LIVES: &str = "set_lives";
pub const GET_BID: &str = "get_bid";
pub const ADD_BIDS: &str = "add_bids";
pub const GET_ROUNDS: &str = "get_rounds";
pub const GET_MANA: &str = "get_mana";
pub const GET_MAX_MANA: &str = "get_max_mana";
pub const GET_MANA_POOL: &str = "get_mana_pool";
pub const ADD_MANA: &str = "add_mana";
pub const SET_MANA: &str = "set_mana";
pub const SET_MAX_MANA: &str = "set_max_mana";
pub const GET_CARDS: &str = "get_cards";
pub const SWITCH_CARDS: &str = "switch_cards";
pub const GET_POWER_CARDS: &str = "get_power_cards";
pub const STEAL_POWER_CARD: &str = "steal_power_card";
pub const DRAW_POWER_CARDS: &str = "draw_power_cards";
pub const PLAYER_IDS: &str = "player_ids";
pub const ADD_MANA_COST: &str = "add_mana_cost";
pub const SET_USABLE: &str = "set_usable";

pub const ON_MATCH_STARTED: &str = "on_match_started";
pub const ON_BID_PLACED: &str = "on_bid_placed";
pub const ON_POWER_CARD_PLAYED: &str = "on_power_card_played";
pub const ON_ROUND_START: &str = "on_round_start";
pub const ON_TURN_PLAYED: &str = "on_turn_played";
pub const ON_ROUND_ENDED: &str = "on_round_ended";
pub const ON_SET_STARTED: &str = "on_set_started";
pub const ON_SET_ENDED: &str = "on_set_ended";

pub const PASSIVE_EVENT_HANDLERS: &[&str] = &[
    ON_MATCH_STARTED,
    ON_BID_PLACED,
    ON_POWER_CARD_PLAYED,
    ON_ROUND_START,
    ON_TURN_PLAYED,
    ON_ROUND_ENDED,
    ON_SET_STARTED,
    ON_SET_ENDED,
];

pub const CARD_RANK_VALUES: &[&str] = &[
    "Four", "Five", "Six", "Seven", "Ten", "Eleven", "Twelve", "One", "Two", "Three",
];

pub const CARD_SUIT_VALUES: &[&str] = &["Golds", "Swords", "Cups", "Clubs"];

pub const POWER_CARD_TYPE_VALUES: &[&str] = &["instant", "targetable", "interactive"];

pub const CARD_FIELDS: &[LuaFieldDefinition] = &[
    LuaFieldDefinition {
        name: "rank",
        lua_type: "CardRank",
        description: "Rank of the visible card.",
    },
    LuaFieldDefinition {
        name: "suit",
        lua_type: "CardSuit",
        description: "Suit of the visible card.",
    },
];

pub const POWER_CARD_FIELDS: &[LuaFieldDefinition] = &[
    LuaFieldDefinition {
        name: "id",
        lua_type: "string",
        description: "Power card identifier.",
    },
    LuaFieldDefinition {
        name: "mana_cost",
        lua_type: "integer",
        description: "Mana cost of the card.",
    },
    LuaFieldDefinition {
        name: "owner_id",
        lua_type: "PlayerId",
        description: "Player that owns the card.",
    },
    LuaFieldDefinition {
        name: "target_player_id",
        lua_type: "PlayerId?",
        description: "Selected target player, when the card has one.",
    },
];

pub const MERCENARY_FIELDS: &[LuaFieldDefinition] = &[
    LuaFieldDefinition {
        name: "id",
        lua_type: "string",
        description: "Mercenary identifier.",
    },
    LuaFieldDefinition {
        name: "owner_id",
        lua_type: "PlayerId",
        description: "Player that owns the mercenary.",
    },
    LuaFieldDefinition {
        name: "base_life",
        lua_type: "integer",
        description: "Configured base life total for this mercenary.",
    },
    LuaFieldDefinition {
        name: "initial_mana",
        lua_type: "integer",
        description: "Configured initial mana pool size for this mercenary.",
    },
];

pub const POWER_CARD_STATE_FIELDS: &[LuaFieldDefinition] = &[
    LuaFieldDefinition {
        name: "id",
        lua_type: "string",
        description: "Power card identifier.",
    },
    LuaFieldDefinition {
        name: "name",
        lua_type: "string",
        description: "Display name of the power card.",
    },
    LuaFieldDefinition {
        name: "description",
        lua_type: "string",
        description: "Display rules text for the power card.",
    },
    LuaFieldDefinition {
        name: "mana_cost",
        lua_type: "integer",
        description: "Mana cost of the power card.",
    },
    LuaFieldDefinition {
        name: "type",
        lua_type: "PowerCardType",
        description: "Play style for the power card.",
    },
    LuaFieldDefinition {
        name: "image_url",
        lua_type: "string?",
        description: "Public image URL, when the card has one.",
    },
    LuaFieldDefinition {
        name: "usable",
        lua_type: "boolean",
        description: "Whether the card is currently enabled by its script.",
    },
];

pub const GAME_METHODS: &[LuaMethodDefinition] = &[
    LuaMethodDefinition {
        name: GET_LIVES,
        params: &[LuaParameterDefinition {
            name: "player_id",
            lua_type: "PlayerId",
        }],
        returns: &["integer"],
        description: "Returns the current life total for a player.",
    },
    LuaMethodDefinition {
        name: ADD_LIVES,
        params: &[
            LuaParameterDefinition {
                name: "player_id",
                lua_type: "PlayerId",
            },
            LuaParameterDefinition {
                name: "delta",
                lua_type: "integer",
            },
        ],
        returns: &["integer"],
        description: "Adds or removes lives from a player and returns the new total.",
    },
    LuaMethodDefinition {
        name: SET_LIVES,
        params: &[
            LuaParameterDefinition {
                name: "player_id",
                lua_type: "PlayerId",
            },
            LuaParameterDefinition {
                name: "lifes",
                lua_type: "integer",
            },
        ],
        returns: &["integer"],
        description: "Sets a player's lives and returns the new total.",
    },
    LuaMethodDefinition {
        name: GET_BID,
        params: &[LuaParameterDefinition {
            name: "player_id",
            lua_type: "PlayerId",
        }],
        returns: &["integer?"],
        description: "Returns the player's current bid, or nil when no bid exists.",
    },
    LuaMethodDefinition {
        name: ADD_BIDS,
        params: &[
            LuaParameterDefinition {
                name: "player_id",
                lua_type: "PlayerId",
            },
            LuaParameterDefinition {
                name: "bid_count",
                lua_type: "integer",
            },
        ],
        returns: &[],
        description: "Adds to the player's bid when a bid is present.",
    },
    LuaMethodDefinition {
        name: GET_ROUNDS,
        params: &[LuaParameterDefinition {
            name: "player_id",
            lua_type: "PlayerId",
        }],
        returns: &["integer"],
        description: "Returns how many rounds the player has won in the current set.",
    },
    LuaMethodDefinition {
        name: GET_MANA,
        params: &[LuaParameterDefinition {
            name: "player_id",
            lua_type: "PlayerId",
        }],
        returns: &["integer"],
        description: "Returns the player's current mana.",
    },
    LuaMethodDefinition {
        name: GET_MAX_MANA,
        params: &[LuaParameterDefinition {
            name: "player_id",
            lua_type: "PlayerId",
        }],
        returns: &["integer"],
        description: "Returns the player's maximum mana.",
    },
    LuaMethodDefinition {
        name: GET_MANA_POOL,
        params: &[LuaParameterDefinition {
            name: "player_id",
            lua_type: "PlayerId",
        }],
        returns: &["integer"],
        description: "Alias for get_max_mana.",
    },
    LuaMethodDefinition {
        name: ADD_MANA,
        params: &[
            LuaParameterDefinition {
                name: "player_id",
                lua_type: "PlayerId",
            },
            LuaParameterDefinition {
                name: "delta",
                lua_type: "integer",
            },
        ],
        returns: &["integer"],
        description: "Adds or removes mana and returns the new current mana.",
    },
    LuaMethodDefinition {
        name: SET_MANA,
        params: &[
            LuaParameterDefinition {
                name: "player_id",
                lua_type: "PlayerId",
            },
            LuaParameterDefinition {
                name: "mana",
                lua_type: "integer",
            },
        ],
        returns: &["integer"],
        description: "Sets current mana, capped by max mana, and returns it.",
    },
    LuaMethodDefinition {
        name: SET_MAX_MANA,
        params: &[
            LuaParameterDefinition {
                name: "player_id",
                lua_type: "PlayerId",
            },
            LuaParameterDefinition {
                name: "mana",
                lua_type: "integer",
            },
        ],
        returns: &["integer"],
        description: "Sets max mana, caps current mana, and returns the new max.",
    },
    LuaMethodDefinition {
        name: GET_CARDS,
        params: &[LuaParameterDefinition {
            name: "player_id",
            lua_type: "PlayerId",
        }],
        returns: &["Card[]"],
        description: "Returns the player's visible normal cards.",
    },
    LuaMethodDefinition {
        name: SWITCH_CARDS,
        params: &[
            LuaParameterDefinition {
                name: "first_player_id",
                lua_type: "PlayerId",
            },
            LuaParameterDefinition {
                name: "first_card",
                lua_type: "Card",
            },
            LuaParameterDefinition {
                name: "second_player_id",
                lua_type: "PlayerId",
            },
            LuaParameterDefinition {
                name: "second_card",
                lua_type: "Card",
            },
        ],
        returns: &["boolean"],
        description: "Switches two visible cards between different players.",
    },
    LuaMethodDefinition {
        name: GET_POWER_CARDS,
        params: &[LuaParameterDefinition {
            name: "player_id",
            lua_type: "PlayerId",
        }],
        returns: &["PowerCardState[]"],
        description: "Returns the player's visible power cards.",
    },
    LuaMethodDefinition {
        name: STEAL_POWER_CARD,
        params: &[
            LuaParameterDefinition {
                name: "from_player_id",
                lua_type: "PlayerId",
            },
            LuaParameterDefinition {
                name: "card_id",
                lua_type: "string",
            },
            LuaParameterDefinition {
                name: "to_player_id",
                lua_type: "PlayerId",
            },
        ],
        returns: &["boolean"],
        description: "Moves a power card from one player to another.",
    },
    LuaMethodDefinition {
        name: DRAW_POWER_CARDS,
        params: &[
            LuaParameterDefinition {
                name: "player_id",
                lua_type: "PlayerId",
            },
            LuaParameterDefinition {
                name: "count",
                lua_type: "integer",
            },
        ],
        returns: &["PowerCardState[]"],
        description: "Draws power cards into a player's visible power card hand and returns them.",
    },
    LuaMethodDefinition {
        name: PLAYER_IDS,
        params: &[],
        returns: &["PlayerId[]"],
        description: "Returns all player IDs known to the script.",
    },
];

pub const POWER_CARD_METHODS: &[LuaMethodDefinition] = &[LuaMethodDefinition {
    name: ADD_MANA_COST,
    params: &[LuaParameterDefinition {
        name: "delta",
        lua_type: "integer",
    }],
    returns: &["integer"],
    description: "Adds to the executing card's mana cost and returns the new effective cost. Negative effective costs regenerate mana.",
}];

pub const POWER_CARD_STATE_METHODS: &[LuaMethodDefinition] = &[LuaMethodDefinition {
    name: SET_USABLE,
    params: &[LuaParameterDefinition {
        name: "usable",
        lua_type: "boolean",
    }],
    returns: &[],
    description: "Enables or disables every copy of this definition in the current player's hand.",
}];

pub const CARD_TYPE: LuaTypeDefinition = LuaTypeDefinition {
    name: "Card",
    description: "A normal visible playing card.",
    fields: CARD_FIELDS,
    methods: &[],
};

pub const POWER_CARD_TYPE: LuaTypeDefinition = LuaTypeDefinition {
    name: "PowerCard",
    description: "The power card currently being executed.",
    fields: POWER_CARD_FIELDS,
    methods: POWER_CARD_METHODS,
};

pub const MERCENARY_TYPE: LuaTypeDefinition = LuaTypeDefinition {
    name: "Mercenary",
    description: "The mercenary whose passive script is running.",
    fields: MERCENARY_FIELDS,
    methods: &[],
};

pub const POWER_CARD_STATE_TYPE: LuaTypeDefinition = LuaTypeDefinition {
    name: "PowerCardState",
    description: "A visible power card in a player's hand.",
    fields: POWER_CARD_STATE_FIELDS,
    methods: POWER_CARD_STATE_METHODS,
};

pub const GAME_TYPE: LuaTypeDefinition = LuaTypeDefinition {
    name: "Game",
    description: "Limited mutable game state available to Lua scripts.",
    fields: &[],
    methods: GAME_METHODS,
};

pub const TYPE_DEFINITIONS: &[LuaTypeDefinition] = &[
    CARD_TYPE,
    POWER_CARD_TYPE,
    MERCENARY_TYPE,
    POWER_CARD_STATE_TYPE,
    GAME_TYPE,
];

pub const MATCH_STARTED_EVENT_FIELDS: &[LuaFieldDefinition] = &[LuaFieldDefinition {
    name: "type",
    lua_type: "\"match_started\"",
    description: "Event discriminator.",
}];

pub const BID_PLACED_EVENT_FIELDS: &[LuaFieldDefinition] = &[
    LuaFieldDefinition {
        name: "type",
        lua_type: "\"bid_placed\"",
        description: "Event discriminator.",
    },
    LuaFieldDefinition {
        name: "player_id",
        lua_type: "PlayerId",
        description: "Player that placed the bid.",
    },
    LuaFieldDefinition {
        name: "bid",
        lua_type: "integer",
        description: "Bid value that was placed.",
    },
];

pub const POWER_CARD_PLAYED_EVENT_FIELDS: &[LuaFieldDefinition] = &[
    LuaFieldDefinition {
        name: "type",
        lua_type: "\"power_card_played\"",
        description: "Event discriminator.",
    },
    LuaFieldDefinition {
        name: "player_id",
        lua_type: "PlayerId",
        description: "Player that played the power card.",
    },
    LuaFieldDefinition {
        name: "card_id",
        lua_type: "string",
        description: "Power card identifier.",
    },
    LuaFieldDefinition {
        name: "target_player_id",
        lua_type: "PlayerId?",
        description: "Selected target player, when present.",
    },
];

pub const TURN_PLAYED_EVENT_FIELDS: &[LuaFieldDefinition] = &[
    LuaFieldDefinition {
        name: "type",
        lua_type: "\"turn_played\"",
        description: "Event discriminator.",
    },
    LuaFieldDefinition {
        name: "player_id",
        lua_type: "PlayerId",
        description: "Player that played a turn.",
    },
    LuaFieldDefinition {
        name: "card",
        lua_type: "Card",
        description: "Normal card that was played.",
    },
];

pub const ROUND_START_EVENT_FIELDS: &[LuaFieldDefinition] = &[LuaFieldDefinition {
    name: "type",
    lua_type: "\"round_start\"",
    description: "Event discriminator.",
}];

pub const ROUND_ENDED_EVENT_FIELDS: &[LuaFieldDefinition] = &[LuaFieldDefinition {
    name: "type",
    lua_type: "\"round_ended\"",
    description: "Event discriminator.",
}];

pub const SET_STARTED_EVENT_FIELDS: &[LuaFieldDefinition] = &[LuaFieldDefinition {
    name: "type",
    lua_type: "\"set_started\"",
    description: "Event discriminator.",
}];

pub const SET_ENDED_EVENT_FIELDS: &[LuaFieldDefinition] = &[LuaFieldDefinition {
    name: "type",
    lua_type: "\"set_ended\"",
    description: "Event discriminator.",
}, LuaFieldDefinition {
    name: "lost_players",
    lua_type: "PlayerId[]",
    description: "Players whose life total decreased during the set.",
}];

pub const EVENT_DEFINITIONS: &[LuaEventDefinition] = &[
    LuaEventDefinition {
        name: "MatchStartedEvent",
        description: "Passive event emitted when a match starts.",
        fields: MATCH_STARTED_EVENT_FIELDS,
    },
    LuaEventDefinition {
        name: "BidPlacedEvent",
        description: "Passive event emitted after a bid is placed.",
        fields: BID_PLACED_EVENT_FIELDS,
    },
    LuaEventDefinition {
        name: "PowerCardPlayedEvent",
        description: "Passive event emitted after a power card is played.",
        fields: POWER_CARD_PLAYED_EVENT_FIELDS,
    },
    LuaEventDefinition {
        name: "TurnPlayedEvent",
        description: "Passive event emitted after a normal card is played.",
        fields: TURN_PLAYED_EVENT_FIELDS,
    },
    LuaEventDefinition {
        name: "RoundStartEvent",
        description: "Passive event emitted when a round starts.",
        fields: ROUND_START_EVENT_FIELDS,
    },
    LuaEventDefinition {
        name: "RoundEndedEvent",
        description: "Passive event emitted when a round ends.",
        fields: ROUND_ENDED_EVENT_FIELDS,
    },
    LuaEventDefinition {
        name: "SetStartedEvent",
        description: "Passive event emitted when a set starts.",
        fields: SET_STARTED_EVENT_FIELDS,
    },
    LuaEventDefinition {
        name: "SetEndedEvent",
        description: "Passive event emitted when a set ends.",
        fields: SET_ENDED_EVENT_FIELDS,
    },
];

pub const PASSIVE_HANDLERS: &[LuaPassiveHandlerDefinition] = &[
    LuaPassiveHandlerDefinition {
        name: ON_MATCH_STARTED,
        event_type: "match_started",
        event_class: "MatchStartedEvent",
        description: "Runs when a match starts.",
    },
    LuaPassiveHandlerDefinition {
        name: ON_BID_PLACED,
        event_type: "bid_placed",
        event_class: "BidPlacedEvent",
        description: "Runs after a bid is placed.",
    },
    LuaPassiveHandlerDefinition {
        name: ON_POWER_CARD_PLAYED,
        event_type: "power_card_played",
        event_class: "PowerCardPlayedEvent",
        description: "Runs after a power card is played.",
    },
    LuaPassiveHandlerDefinition {
        name: ON_ROUND_START,
        event_type: "round_start",
        event_class: "RoundStartEvent",
        description: "Runs when a round starts.",
    },
    LuaPassiveHandlerDefinition {
        name: ON_TURN_PLAYED,
        event_type: "turn_played",
        event_class: "TurnPlayedEvent",
        description: "Runs after a normal card is played.",
    },
    LuaPassiveHandlerDefinition {
        name: ON_ROUND_ENDED,
        event_type: "round_ended",
        event_class: "RoundEndedEvent",
        description: "Runs when a round ends.",
    },
    LuaPassiveHandlerDefinition {
        name: ON_SET_STARTED,
        event_type: "set_started",
        event_class: "SetStartedEvent",
        description: "Runs when a set starts.",
    },
    LuaPassiveHandlerDefinition {
        name: ON_SET_ENDED,
        event_type: "set_ended",
        event_class: "SetEndedEvent",
        description: "Runs when a set ends.",
    },
];

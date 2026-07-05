return {
    id = "strike_10",
    name = "Strike 10",
    description = "Remove 10 lives from a target player.",
    requires_target = true,
    effect = function(game, card)
        game.add_lives(card.target_player_id, -10)
    end,
}

return {
    effect = function(game, card)
        game.add_lives(card.target_player_id, -10)
    end,
}

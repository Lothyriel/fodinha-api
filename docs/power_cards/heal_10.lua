return {
    effect = function(game, card)
        game.add_lives(card.owner_id, 10)
    end,
}

return {
    effect = function(game, card)
        game.add_lives(card.targets[1], -10)
    end,
}

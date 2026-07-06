return {
    id = "heal_10",
    name = "Heal 10",
    description = "Restore 10 lives to yourself.",
    type = "instant",
    effect = function(game, card)
        game.add_lives(card.owner_id, 10)
    end,
}

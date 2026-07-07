db.adminCommand("listDatabases").databases
  .map(d => d.name)
  .filter(name => name.startsWith("oh_hell_test"))
  .forEach(name => db.getSiblingDB(name).dropDatabase())

<?

println(uuid())




Database connection = Database(":memory:")
bool result = connection.execute("
  CREATE TABLE IF NOT EXISTS users (name TEXT, age INTEGER);
  INSERT INTO users VALUES ('Alice', 42);
  INSERT INTO users VALUES ('Bob', NULL);
")

println("Insertion: " + result)

Statement stmt = connection.prepare("SELECT * FROM users WHERE age = ?")
stmt.bind<int>(1, 42)

while stmt.next()
  print(stmt.read<string>("name") + "\n")
  print(stmt.read<int>("age") + "\n")
<?
Map<string, string> query = parseQuery("a=b&b=c&something_else=another-thing&notright#&blank=&ignoreme!")
println(query["blank"])


println(uuid())

Database connection = Database(":memory:")
bool result = connection.execute("
  CREATE TABLE IF NOT EXISTS users (name TEXT, age INTEGER, data BLOB);
  INSERT INTO users VALUES ('Alice', 42, x'6869');
")

println("Insertion: " + result)

Statement stmt = connection.prepare("INSERT INTO users(name, age, data) VALUES (?, ?, ?)")
stmt.bind<string>(1, "Frank")
stmt.bind<int>(2, 42)
stmt.bind<char[]>(3, ['a', 'b'])
stmt.next()

stmt = connection.prepare("SELECT * FROM users WHERE age = ?")
stmt.bind<int>(1, 42)

while stmt.next()
  print(stmt.read<string>("name") + "\n")
  print(stmt.read<int>("age") + "\n")
  print(stmt.read<char[]>("data").toString() + "\n")
  println("")
<?
Map<string, string> queries = parseQuery("a=b&b=c&something_else=Computer%25252525252BGraphics%2525252525252C%25252525252BRaytracing&notright#&blank=&ignoreme!&=apple")

println(queries.contains("a") + "")
println(queries.contains("") + queries[""])
println(urlDecode("# hi<b></b> %E2%80%94") + query())

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
stmt.bind<string>(3, "ab")
stmt.next()

stmt = connection.prepare("SELECT * FROM users WHERE age = ?")
stmt.bind<int>(1, 42)

while stmt.next()
  print(stmt.read<string>("name") + "\n")
  print(stmt.read<int>("age") + "\n")
  print(stmt.read<string>("data") + "\n")
  println("")
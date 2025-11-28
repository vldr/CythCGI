hello world

<?

int[] a = [1,2,3]
print(body())

header("Poop: " + uuid())

Database connection = Database(":memory:")
bool result = connection.execute("
  CREATE TABLE IF NOT EXISTS users (name TEXT, age INTEGER, data BLOB);
  INSERT INTO users VALUES ('Alice', 42, x'6869');
")

print((string)result)

?>bye
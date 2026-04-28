<?

JsonObject obj = JsonObject()
obj["a"] = 10
obj["b"] = true
obj["c"] = "hello world"

JsonArray arr = JsonArray()
arr.push("hello world")
arr.push(10)
arr.push(true)

obj["d"] = arr

print(jsonEncode(obj))
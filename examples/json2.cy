<?

JsonObject obj = JsonObject()
obj["a"] = 10
obj["b"] = true
obj["c"] = "hello world"

JsonArray arr = JsonArray()
arr.push("hello world")
arr.push(12)
arr.push(10.5)
arr.push(true)
arr.push(false)

obj["d"] = arr

print(jsonEncode(obj))
<?

FetchResult data = fetch("https://trackr.vldr.org/api/values/?apiKey=VBjOXOncKt4yMkyMDsN4964Utbxb4sdGUuijOscId246uTiTljodn9wd29P2zSGe&fieldId=1&order=asc&offset=0&limit=0", 
                        FetchOptions()
                        .method("GET")
                        .header("Cool", "Header")
                    )

any a = jsonDecode(data.body)
JsonObject obj = (JsonObject)a
JsonArray arr = (JsonArray)obj["values"]

float average
float max

for any item in arr.value
  JsonObject obj = (JsonObject)item
  JsonString val = (JsonString)obj["value"]
  float num = parseFloat(val.value) 
  average += num
  max = num > max ? num : max

print("Average " + average / arr.value.length +
      " Max " + max)

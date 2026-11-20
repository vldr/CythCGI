<?

FetchResult data = fetch("https://trackr.vldr.org/api/values/?apiKey=VBjOXOncKt4yMkyMDsN4964Utbxb4sdGUuijOscId246uTiTljodn9wd29P2zSGe&fieldId=1&order=asc&offset=0&limit=0", 
                        FetchOptions()
                        .method("GET")
                        .header("Cool", "Header")
                    )

JsonValue root = jsonDecode(data.body)
JsonObject obj = (JsonObject)root
JsonArray array = (JsonArray)obj["values"]

float average
float max

for JsonValue item in array
  JsonObject obj = (JsonObject)item
  JsonString val = (JsonString)obj["value"]

  float num = parseFloat(val) 
  average += num
  max = num > max ? num : max

print("Average " + average / array.length +
      " Max " + max)

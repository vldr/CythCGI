<?
string data = fetch("https://countriesnow.space/api/v0.1/countries/population/cities")

any a = jsonDecode(data)

println(jsonEncode(a))

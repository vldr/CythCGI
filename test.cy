<?

void printer()
  string[] list = ["hello", "bye", "ciao"]
  for int i = 0; i < list.length; i += 1
    
  
  
    print("<h1>" + list[i] + "</h1>\n")

?>

<!DOCTYPE html>
<html>
  <head>
    <title><?print("Hello World")?><? print(date(now(), "%Y-%m-%d %H:%M:%S"))?></title>
  </head>
  <body>
    <?
      string[] list = ["hello", "bye", "ciao"]
      for int i = 0; i < list.length; i += 1
        print("<h1>" + list[i] + "</h1>\n")
    ?>
  </body>
</html>

<?





  printer()?><?

  
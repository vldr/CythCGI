<!DOCTYPE html>
<html>
  <head>
    <title>Page Title</title>
  </head>
  <body>
    <?
      string[] list = [ "Hello World", "Foobar", "Goobar"]

      for int i = 0; i < list.length; i += 1
        print("<h1>" + list[i] + "</h1>\n")
    ?>
  </body>
</html>
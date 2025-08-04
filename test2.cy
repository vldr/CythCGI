<?

  float map(float value, float inMin, float inMax, float outMin, float outMax)

    return outMin + (outMax - outMin) * (value - inMin) / (inMax - inMin)



  int height = 500

  int width = 500



  float w = 3.0

  float h = (w * height) / width



  float minX = -w / 1.5

  float maxX = minX + w



  float minY = -h / 2.0

  float maxY = minY + h

?>



<canvas id="canvas"></canvas>

<script>

  const width = <? print((string)width) ?>;

  const height = <? print((string)height) ?>;



  const pixels = [

    <?

      for int y = 0; y < width; y += 1

        for int x = 0; x < height; x += 1

          float a = map((float)x, 0.0, (float)width, minX, maxX)

          float b = map((float)y, 0.0, (float)height, minY, maxY)

    

          float ca = a

          float cb = b

    

          float n = 0.0

          float maxIterations = 100.0

    

          while n < maxIterations

            float aa = a * a - b * b

            float bb = 2 * a * b

            a = aa + ca

            b = bb + cb

    

            if a * a + b * b > 64

              break

    

            n += 1.0

    

          float bright = map(n, 0.0, maxIterations, 0.0, 1.0)

          bright = map(bright.sqrt() * 2.0, 0.0, 1.0, 0.0, 255.0)

    

          if n == maxIterations

            bright = 0.0



          print((int)bright + ", ")

        print("\n\n")?>

  ];



  const rgba = new Uint8ClampedArray(width * height * 4);

  for (let i = 0; i < pixels.length; i++) {

    const color = pixels[i];

    const j = i * 4;

    rgba[j + 0] = (color >> 16) & 0xFF;

    rgba[j + 1] = (color >> 8) & 0xFF; 

    rgba[j + 2] = color & 0xFF;        

    rgba[j + 3] = 255;                 

  }



  const canvas = document.getElementById('canvas');

  canvas.width = width;

  canvas.height = height;

  const ctx = canvas.getContext('2d');

  const imageData = new ImageData(rgba, width, height);

  ctx.putImageData(imageData, 0, 0);

</script>


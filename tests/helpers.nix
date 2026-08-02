{
  raw = ./assets/small_raw.NEF;
  xmp =
    file_name: rating:
    let
      xmp_text = builtins.readFile ./assets/small_raw.NEF.xmp;
      text = builtins.replaceStrings [ "<FILENAME>" "<RATING>" "<EXTENSION>"] [file_name (toString rating) "NEF"] xmp_text;
    in
    builtins.toFile file_name text;
}

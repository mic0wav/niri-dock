# Niri Dock
 this name is not final


This is a simple dock made in rust for niri, it uses gtk4 and layer-shell

## installing and running

```
$ git clone https://github.com/mic0wav/niri-dock --depth=1
$ cd dock

$ cargo r # to test if it works

$ cargo b --release
$ ./target/release/dock # for the optimized binary

$ cargo install --path . # for user wide installation
```



## configuration

Place config.toml and dock.css in XDG-CONFIG-HOME/dock or in ~/.config/dock

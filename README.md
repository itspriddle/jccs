# jccs - Josh's Claude Code Statusline

Claude Code statuslines are a dime a dozen. This one is mine. Use it or don't.

![](docs/assets/screenshot-v0.4.0.png)

## Install

With Homebrew:

```sh
brew install itspriddle/brews/jccs
```

Or manually:

```sh
git clone https://github.com/itspriddle/jccs
cd jccs
make build
sudo make install
```

Setup Claude Code:

```
jccs install
$EDITOR ~/.config/jccs/config.toml
```

## Uninstall

```sh
jccs uninstall
sudo rm /usr/local/bin/jccs
```

## License

[MIT](LICENSE)

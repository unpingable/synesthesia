Output docs/tcp-kernel-weather.gif
Set Shell "bash"
Set FontSize 14
Set Width 1000
Set Height 600
Set Framerate 24
Set TypingSpeed 8ms
Set Padding 10

Hide
Type "export PS1=''; clear"
Enter
Sleep 500ms
Show
Type "env -u NO_COLOR TERM=xterm-256color COLORTERM=truecolor ./target/release/synesthesia replay examples/tcp-kernel-weather.ndjson --speed 2 --mode ansi --theme cold --view weather"
Enter
Sleep 9s

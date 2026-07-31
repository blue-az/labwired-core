# Ping Pong — two ESP32-C3s, one screen

Two boards rally a ball back and forth over a plain serial wire. No host, no
WiFi, no coordinator: each board keeps the other running. The second board draws
the match on a 128x64 OLED — ball, paddles, live rally count, best rally.

Both sketches are ordinary Arduino. The only library is `Wire`, which ships with
the ESP32 core, so there is nothing to install.

## Wiring

Cross the serial link (each board's TX to the other's RX) and **share ground** —
without a common ground neither board sees a byte, which is the single most
common way this fails on a desk.

```
  Player A                    Player B (+ OLED)
  GPIO6 TX  ---------------->  GPIO7 RX
  GPIO7 RX  <----------------  GPIO6 TX
  GND       -----------------  GND

                               OLED SDA -> GPIO4
                               OLED SCL -> GPIO5
                               OLED VCC -> 3V3
                               OLED GND -> GND
```

`Serial1` is the link; `Serial` (UART0) stays the USB console on both boards, so
rally traffic never collides with the messages you read over USB.

## Flashing

| Board | Sketch |
|---|---|
| Player A (server) | `server.ino` |
| Player B (screen) | `screen.ino` |

Order does not matter. Player A serves immediately and re-serves after a
one-second timeout, so whichever board boots second simply joins the rally.

## What to expect

Player A's console counts rallies; Player B's counts returns and the OLED
animates one step per received ball — the picture is driven by the actual rally,
not by a timer, so if the link breaks the ball stops moving.

Pull the link wire and Player A reports `missed - rally ended at N`, then serves
again. Reconnect and the rally resumes.

## Ideas to fork

- Make it a real game: drop the ball if a return takes too long, and keep score.
- Add a button so a human can serve.
- Three boards in a ring, passing the ball on.
- Print the rally over USB and plot it on a host.

# telegram-lviv-flat-rent-bot

Watches Telegram channels and bots for flat-rent offers in Lviv (Франківський district), filters them,
and sends matches to one Telegram chat. No database: state is a JSON file in `state/`.

## Status

- [x] M1: config, notifier, Containerfile
- [x] M2: read channels and bot chats with a Telegram user account (grammers)
- [x] M3: parse price / rooms / floor / heating / street, filter, dedupe, short card per flat
- [x] Photos from bot sources (album), link preview for channel posts
- [x] LUN (lun.ua) district page every 15 min: structured cards with phone, photos, direct link; OLX
      listings skipped (@homierentbot covers OLX)

LUN has no API; the watcher reads the cards LUN embeds in its server-rendered page (a path `robots.txt`
allows, no query string), one request per 15 min. Zone check uses the listing's coordinates.

Sources are polled (`POLL_SECS`, default 60) by reading only the chats listed in `config.toml`; the account's
live update stream is dropped unread. For a bot source (e.g. @homierentbot) start it and set its filters in
Telegram first; the watcher only reads what the bot sends you.

## Run locally

```bash
cp .env.example .env   # fill all four values
cargo run -- login     # once: phone, code, 2FA password -> state/tg-session.json
cargo run
```

`state/tg-session.json` gives full access to the Telegram account. Never commit or share it.

## Container (Podman)

```bash
podman build -t flat-rent-bot -f Containerfile .
podman run -it --rm --env-file .env -v ./state:/app/state flat-rent-bot /app/telegram-lviv-flat-rent-bot login
podman run -d --name flat-rent-bot --env-file .env -v ./state:/app/state flat-rent-bot
```

## Filter

`config.toml`: price, rooms, floor. Street zone: `data/streets.json` (listed streets + adjacent ones inside
their hull, from OpenStreetMap), map in `docs/zone.html`. A post naming only "Франківський район" passes
with a note that the street is not given.

## License

Source-available, see [LICENSE](LICENSE).

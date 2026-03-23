# Yoink Cookie Sync - Browser Extension

Sends your browser cookies to Yoink bot so it can download authenticated content (TikTok, Instagram, YouTube, etc.) on your behalf.

## How it works

1. In Telegram, send `/cookie token` to your Yoink bot - it replies with a short-lived single-use token (10 min TTL)
2. Paste the token into the extension popup
3. Extension reads cookies for configured domains via `chrome.cookies` API (includes httpOnly)
4. Cookies are posted to your Yoink instance in Netscape format and stored per-user in the database
5. Bot uses your cookies for yt-dlp downloads until they expire

The token is single-use and short-lived - no persistent auth is stored in the extension.

## Installation (Chrome / Chromium / Edge)

1. Open `chrome://extensions`
2. Enable **Developer mode** (top right toggle)
3. Click **Load unpacked**
4. Select the `browser-extension/` directory from the yoink-core repo

## Usage

1. Open the extension popup
2. Set **Bot URL** to your Yoink instance (e.g. `https://yoink.example.com`)
3. Make sure you're logged into TikTok / YouTube / etc. in this browser
4. Send `/cookie token` to your bot, copy the token
5. Paste the token and click **Send Cookies**

## Domains

Default: `youtube.com`, `tiktok.com`, `instagram.com`, `x.com`, `twitter.com`

Add/remove domains in the popup. Use **+Current** to add the domain of the currently open tab.

## Security

- Cookies are sent directly to your own Yoink server over HTTPS
- Token is single-use and expires in 10 minutes
- No third-party servers involved

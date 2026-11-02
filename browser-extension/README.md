# Yoink Cookie Sync

This Chromium extension sends selected browser cookies to your Yoink instance. The bot can use those credentials for authenticated media requests.

Cookies can grant account access. Use this extension only with an instance and operator you trust.

## Install

1. Open `chrome://extensions` in Chrome, Chromium, or Edge.
2. Enable Developer mode.
3. Select Load unpacked.
4. Select this `browser-extension/` directory.

## Send cookies

1. Sign in to the required provider in this browser.
2. Open the extension popup.
3. Set Bot URL to your instance's HTTPS origin.
4. Send `/cookie token` to your bot.
5. Paste the token into the popup.
6. Select Send Cookies.

The token expires after ten minutes and permits one successful submission. Validation or authorization failure does not constitute successful consumption.

The extension reads selected cookies, including HttpOnly cookies, through the browser API. It submits Netscape-format content to `/api/v1/dl/cookies/submit`.

## Domains

Defaults are `youtube.com`, `tiktok.com`, `instagram.com`, and `x.com`.

Edit the list in the popup. Use +Current to add the active tab's domain.

## Security and limits

Use HTTPS. The popup does not enforce the scheme, so entering an HTTP URL can expose the token and cookie contents.

The popup stores the Bot URL and domain list in browser sync storage. It does not persist the submission token.

Cookies expire or become invalid when a provider changes a session. Submission does not guarantee that a provider will accept a later download.

See [cookie contracts](../docs/features/downloads/cookies.md) for authorization, limits, and replacement behavior.

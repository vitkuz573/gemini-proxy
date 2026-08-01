# Reverse-engineering: model category IDs from BardChatUi modules

**Date:** 2026-08-01

## Goal
Map the numeric value sent in `inner_req_list[30]` to model mode categories so we can
reliably request Fast, Pro, Thinking, Auto, etc. through the cookie-auth path without
resorting to browser-side picking.

## Findings from `BardChatUi_modules.js`

The minified JS contains an obvious enum mapper:

```js
_.EG=function(a){
  switch(a){
    case 1:return"MODE_CATEGORY_FAST";
    case 2:return"MODE_CATEGORY_THINKING";
    case 5:return"MODE_CATEGORY_FAST_DYNAMIC_THINKING";
    case 3:return"MODE_CATEGORY_PRO";
    case 4:return"MODE_CATEGORY_AUTO";
    case 6:return"MODE_CATEGORY_FLASH_LITE";
    default:return"MODE_CATEGORY_UNSPECIFIED"
  }
};
```

A corresponding reverse function exists for the protobuf layer:

```js
_.puc=function(a){
  switch(a){
    case "THINKING_LEVEL_STANDARD":return 1;
    case "THINKING_LEVEL_EXTENDED":return 2;
    case "THINKING_LEVEL_DEEP_THINK":return 3;
    default:return 0
  }
};
```

Model records include a `zr` field populated by `_.EG(b.xE())`, where `xE()` reads the
mode-category field from a proto message.  This confirms the category is a small integer
passed through the protobuf stack.

## Relationship to our 97-slot request

In all captured cookie-auth requests `inner_req_list[30]` equals `[4]`.  The mapper above
gives `4 -> MODE_CATEGORY_AUTO`, which matches the default behavior when no explicit model
is selected in the Gemini web UI (the "auto" model picker).

| Value | Category                        | Likely mapping to our slot 30 |
|-------|---------------------------------|-------------------------------|
| 1     | `MODE_CATEGORY_FAST`            | `[1]`                         |
| 2     | `MODE_CATEGORY_THINKING`        | `[2]`                         |
| 3     | `MODE_CATEGORY_PRO`             | `[3]`                         |
| 4     | `MODE_CATEGORY_AUTO`            | `[4]` (verified)              |
| 5     | `MODE_CATEGORY_FAST_DYNAMIC_THINKING` | `[5]`                   |
| 6     | `MODE_CATEGORY_FLASH_LITE`      | `[6]`                         |

## Hardcoded model IDs in the same modules

The JS also contains hardcoded 16-hex model IDs referenced by feature flags.  The list
likely represents the currently available model fleet:

```
a74ec8485b3b5ce4, 8a338031b4db9317, b11155c88d2cdac8, 1640bdc9f7ef4826,
cf41b0e0dd7d53e5, 8c46e95b1a07cecc, 1d44b34bcaa1c04d, 203e6bb81620bcfe,
9d60dfae93c9ff1f, 7daceb7ef88130f5, 2525e3954d185b3c, 1acf3172319789ce,
9d8ca3786ebdfbea, e6fa609c3fa255c0, 797f3d0293f288ad, 948b866104ccf484
```

The IDs `9d8ca3786ebdfbea`, `e6fa609c3fa255c0`, `797f3d0293f288ad` appear in a list right
next to the constants "Gemini 2.5" / "Gemini 2.5 Pro", suggesting they are the Gemini 3
family variants (Gemini 3 Flash, Pro, Thinking).

## Implications

1. We can probably route to Fast/Pro/Thinking/Auto/Flash-Lite by changing only
   `inner_req_list[30]` from `[4]` to `[1..6]` without touching model IDs elsewhere.
2. This is a **small, testable change** and removes a current limitation: the proxy only
   gets whatever model the browser session last used (usually Auto).
3. The finding does **not** solve the attestation problem (headless Chromium still
   required), but it materially improves user control and is much easier to verify than
   token replay or protobuf regeneration.

## Recommended next step

Implement model category selection in the OpenAI → Gemini converter and test it against a
live browser session:

- Map OpenAI `model` parameter to the integer category using the enum above.
- Default to `4` (AUTO) when unspecified.
- Add a small CLI smoke test that sends a request with `model: pro` and checks the
  response metadata for the expected model signature.

## Risk

Google may perform additional server-side checks beyond slot 30 (e.g. model picker state
stored in `ST1bg`/`SNlM` cookies).  If slot 30 alone is insufficient, we may need to also
inject the corresponding model ID into `inner_req_list` slots 34/35 or elsewhere.

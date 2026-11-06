# Phone layout

A desktop layout squeezed into one column passes a "nothing is broken" check and
still fails a person holding a phone. The usual failures: a table you scroll
sideways to reach the column that matters, two rows of tabs cut mid-word, the
answer below a map or a wall of filters, a row tap that opens its detail in a side
panel off the screen, and explanations that only open on hover, which a touch
screen never does.

The fix is a **phone view**: a second presentation of the same screen, made from
the phone kit in `@oxy-hq/sdk/shell` (`@oxy-hq/sdk` 2.27.0 and later). It is styled by `@oxy-hq/sdk/shell.css`, which the
app already imports for the shell.

## When to build one

Build a phone view for any screen people open on a phone: anything with a table
wider than a phone, more than about five tabs or sections, a map, side panels, or
hover tooltips. A screen that is already a single column of text and buttons only
needs the floor below (text ≥ 13px, labels ≥ 11px, actions ≥ 44px, fields ≥ 16px).

## The pattern

Keep the desktop component exactly as it is. Render the phone view beside it, fed
by the same hooks and actions; only the presentation changes.

```tsx
import { usePhoneLayout } from "@oxy-hq/sdk/shell";

export function Orders() {
  const data = useOrders();          // the hooks the desktop already uses
  const phone = usePhoneLayout();    // a phone upright, or on its side
  return phone ? <OrdersPhone {...data} /> : <OrdersDesktop {...data} />;
}
```

`usePhoneLayout()` is true below 640px wide, and on a touch screen no taller than
500px (a phone on its side); it re-renders when that changes and is `false` on the
server. Do not branch on the user agent, and do not restyle the desktop component
with breakpoints until it "fits": the desktop and TV layout must not move.

## What each desktop part becomes

| On desktop | On the phone |
| --- | --- |
| Tabs, a sidebar, a section switcher | `PhoneTabBar` at the bottom: up to five; past five it shows four and a More sheet |
| The page | `PhonePage` with `tabBar` (and `stickyAction`); pass the tab id as `scrollKey` |
| KPI cards | `PhoneKpiStrip`, two per row, first on the screen |
| A table | `PhoneCardList` of `PhoneCard`s: the row's name, a status pill, two or three key figures; a `summary` line on top. **Never a sideways-scrolling table.** |
| A row's action button (Post, Approve) | the card's `trailing`, or `PhoneStickyAction` for the screen's one main action |
| A detail panel, drawer or modal | `PhoneSheet` (full screen, Back). The phone's Back closes it through `onClose` and the page stays put; a sheet that opened by pushing its own entry (a route, `?item=`) passes `historyEntry={false}` |
| A list of things that need someone | `PhoneSection` of `PhoneRow`s with a `tone` dot |
| A row of filter selects | one `PhoneFilterChip` opening a `PhoneSheet` of filters |
| List / Map, Day / Week | `PhoneSegmented` |
| A hover tooltip or footnote | `PhoneHint`, an ⓘ that opens on a tap |
| A map beside or above the content | its own tab, or a sheet, filling the screen; never a fixed half above the numbers |
| Device-frame previews, wide editors, fullscreen/kiosk buttons, keyboard hints | left out of the phone view |

## The first screen answers the job

Decide what someone opens this screen for on a phone: "is anything on fire", "what
do I need to post today", "how did yesterday go". That answer goes first, visible
without scrolling: a status row and the KPI strip, then the "needs you" rows. Maps,
history and settings come after, or behind a tab.

## Rules of thumb

- One page scroller. `PhonePage` scrolls with the shell's main area; never put a
  fixed-height box that scrolls inside it. Its dock reaches the bottom of the screen
  through wrappers, but padding on a wrapper insets it: for an edge-to-edge tab bar,
  render `PhonePage` without a padded `<main>` around it.
- Everything a person can tap is at least 44px; the kit's pieces already are.
- Text in a phone view is 13px or more, labels 11px or more, numbers in tabular
  figures. Fields are 16px or more, or iOS zooms the page when one is focused: the
  kit raises `text-sm` and smaller and fields with no size, and leaves larger ones
  alone. It cannot raise `text-[11px]` or your own small `font-size`, so don't set
  those on a field.
- Explanations a tap away (`PhoneHint`), never hover-only.
- Use the kit's colours (it follows your theme tokens and a `.dark` ancestor)
  rather than hard-coded ones, so light and dark both work. Its corners follow
  `--radius` (at `0`, square, with only status dots round) and its text `--font-sans`.

## Checking it

Look at it, don't only test it. With the browser at 390×844 and again at 844×390,
touch emulation on, light and dark: screenshot every tab at the top and after each
screenful of scrolling, open a detail sheet, and read each screenshot as someone who
opened the app to get something done. Then load the desktop at 1440×900 and compare
it with the published version: it must be unchanged.

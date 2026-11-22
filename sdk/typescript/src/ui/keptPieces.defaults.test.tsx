// @vitest-environment jsdom
//
// SDK 2.32.0 (the kept pieces, decision 99) added opt-in props to PanelTable, Subnav, StatGrid,
// PhoneKpiStrip, AlertStrip and Sparkline. This file draws each piece with NONE of them, on a
// desk and on a phone layout, and holds the markup in a snapshot that was written by the 2.31.1
// source (origin/main before the change, `vitest -u`) and is now read by the 2.32.0 source: an app
// that passes none of the new props renders exactly as it did. Never update the snapshot to make
// a change pass; a difference here is a default that moved.

import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PhoneKpiStrip } from "../shell/phone/PhoneKpiStrip";
import { Sparkline } from "./charts/Sparkline";
import { sparkPoints } from "./charts/series";
import { AlertStrip } from "./figures/AlertStrip";
import { Stat, StatGrid } from "./figures/Stat";
import { Subnav } from "./navigation/Subnav";
import { type PanelColumn, PanelTable } from "./tables/PanelTable";
import { render, stubMatchMedia } from "./ui.harness";

afterEach(() => {
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
});

async function markup(node: ReactNode, phone: boolean): Promise<string> {
  vi.stubGlobal("matchMedia", stubMatchMedia(phone).matchMedia);
  const { container, unmount } = await render(node);
  const html = container.innerHTML;
  await unmount();
  return html;
}

interface Plant {
  id: string;
  name: string;
  status: string;
  region: string;
  purity: number;
}
const PLANTS: Plant[] = [
  { id: "p1", name: "Plant 01", status: "Needs action", region: "US West", purity: 93.57 },
  { id: "p2", name: "Plant 02", status: "On track", region: "India", purity: 95.32 }
];
const COLUMNS: PanelColumn<Plant>[] = [
  { header: "Status", cell: (p) => p.status },
  { header: "Plant", cell: (p) => p.name },
  { header: "Region", cell: (p) => p.region },
  { header: "Purity %", align: "right", cell: (p) => p.purity.toFixed(2), total: "94.45" }
];

const CASES: Record<string, () => ReactNode> = {
  "PanelTable, plain": () => (
    <PanelTable caption='Plants' columns={COLUMNS} rows={PLANTS} rowKey={(p) => p.id} />
  ),
  "PanelTable, opening rows, selection, row class, actions, totals": () => (
    <PanelTable
      label='Plants'
      lead='Two plants.'
      footnote='All of them.'
      columns={COLUMNS}
      rows={PLANTS}
      rowKey={(p) => p.id}
      rowHeader={false}
      rowClassName={(p) => (p.id === "p1" ? "subject" : undefined)}
      onRowOpen={() => {}}
      isSelected={(p) => p.id === "p2"}
      actions={(p) => <button type='button'>Open {p.name}</button>}
      totalLabel='Total'
    />
  ),
  Subnav: () => (
    <Subnav
      current='fleet/overview'
      sections={[
        {
          heading: "Fleet",
          items: [{ id: "fleet/overview", label: "Overview", href: "#/fleet/overview" }]
        },
        {
          heading: "Operations",
          items: [
            { id: "ops/plant", label: "Plant", href: "#/ops/plant" },
            { id: "ops/cause", label: "Cause", href: "#/ops/cause" }
          ]
        }
      ]}
      footer={[{ id: "gallery", label: "Gallery", href: "#/gallery" }]}
    />
  ),
  "StatGrid, three tiles at 12rem": () => (
    <StatGrid min='12rem'>
      <Stat boxed label='Purity' value='93.6%' />
      <Stat boxed label='Gap' value='-4.01' />
      <Stat boxed label='Alarms' value='5' />
    </StatGrid>
  ),
  "PhoneKpiStrip, three tiles": () => (
    <PhoneKpiStrip
      items={[
        { label: "Sales", value: "$12.4K", delta: "+8%", tone: "good" },
        { label: "Labour", value: "31%" },
        { label: "Waste", value: "2.1%", onClick: () => {} }
      ]}
    />
  ),
  "AlertStrip, every tone and door": () => (
    <AlertStrip
      alerts={[
        { tone: "error", label: "3 SKUs are out of stock", onSelect: () => {} },
        { tone: "info", label: "1 SKU to check", href: "/check" },
        { tone: "warning", label: "2 SKUs at risk" },
        { tone: "neutral", label: "1 discontinued", ariaLabel: "Discontinued" },
        { tone: "success", label: "All posted" }
      ]}
    />
  ),
  "AlertStrip, all clear": () => <AlertStrip alerts={[]} allClear='Nothing urgent' />,
  "Sparkline, trend with peer": () => (
    <Sparkline
      label='North, food cost'
      tone='warning'
      points={[
        { period: "2026-01", observedPct: 30, peerPct: 28 },
        { period: "2026-02", observedPct: 32, peerPct: 29 },
        { period: "2026-03", observedPct: 31, peerPct: 29 }
      ]}
    />
  ),
  "Sparkline, one series, sized, dot, no area": () => (
    <Sparkline
      label='Active people by week'
      points={sparkPoints([9, 10, 11, 4])}
      color='var(--foreground)'
      area={false}
      end='dot'
      width={150}
      height={36}
    />
  ),
  "Sparkline, too short": () => <Sparkline label='New' points={sparkPoints([3])} />
};

describe("2.32.0 defaults: every piece without the new props draws as 2.31.1 did", () => {
  for (const [name, node] of Object.entries(CASES)) {
    it(`${name}, desk`, async () => {
      expect(await markup(node(), false)).toMatchSnapshot();
    });
    it(`${name}, phone layout`, async () => {
      expect(await markup(node(), true)).toMatchSnapshot();
    });
  }
});

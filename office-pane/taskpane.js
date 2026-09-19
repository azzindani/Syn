/* Syn Office.js hand: Mac + Office-on-the-web.
 * Speaks office-rpc/1 envelopes to the widget over plain fetch; on desktop
 * the same envelopes travel the named pipe via the COM sidecar.
 * Sideload: point manifest.xml at taskpane.html, then Insert > Add-ins.
 * STATUS: scaffold with live read/write paths; run inside Office to verify. */
"use strict";
const PIPE_URL = "http://127.0.0.1:3791/rpc"; // widget relay endpoint (M4)
const out = (t) => { document.getElementById("out").textContent = t; };

async function sendRpc(method, handle, args) {
  const envelope = { jsonrpc: "office-rpc/1", method, handle, args };
  try {
    const r = await fetch(PIPE_URL, {
      method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify(envelope),
    });
    return await r.text();
  } catch (e) {
    return "(widget offline — envelope that would send: " + JSON.stringify(envelope) + ")";
  }
}

Office.onReady(() => {
  document.getElementById("read").onclick = () =>
    Word.run(async (ctx) => {
      const sel = ctx.document.getSelection();
      sel.load("text");
      await ctx.sync();
      out(await sendRpc("read", "word:web-doc:Body", { selector: "body", preview: sel.text.slice(0, 120) }));
    }).catch((e) => out("read failed: " + e.message));

  document.getElementById("write").onclick = () =>
    Word.run(async (ctx) => {
      ctx.document.body.insertParagraph("Syn was here", "End");
      await ctx.sync();
      out(await sendRpc("write", "word:web-doc:Body", { selector: "p-last", payload: "Syn was here" }));
    }).catch((e) => out("write failed: " + e.message));
});

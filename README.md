# a2app-harness


# A2App: Agentic App Building and Runtime System

A2App is a system for building and running agentic applications. It composes an inference endpoint, a shared workspace, an app runtime, and a coordination harness into a coherent whole.

---

## Key Features

Four capabilities define the system, independent of any particular implementation:

<svg viewBox="0 0 700 200" xmlns="http://www.w3.org/2000/svg" width="700" height="200">
  <defs>
    <style>
      .feat-box { fill: #1e2533; stroke: #4a90d9; stroke-width: 1.5; rx: 6; }
      .feat-label { font-family: monospace; font-size: 13px; fill: #e0e8f0; text-anchor: middle; }
      .feat-num { font-family: monospace; font-size: 11px; fill: #4a90d9; text-anchor: middle; }
      .harness-box { fill: none; stroke: #6a4fc9; stroke-width: 2; stroke-dasharray: 6 3; }
      .harness-label { font-family: monospace; font-size: 11px; fill: #9b7de8; text-anchor: middle; }
    </style>
  </defs>

  <!-- Harness background -->
  <rect x="10" y="10" width="680" height="180" rx="8" class="harness-box"/>
  <text x="350" y="175" class="harness-label">4. Harness — coordinates inference, workspace, and runtime</text>

  <!-- Feature 1 -->
  <rect x="30" y="35" width="150" height="110" rx="6" class="feat-box"/>
  <text x="105" y="78" class="feat-num">1</text>
  <text x="105" y="98" class="feat-label">Inference</text>
  <text x="105" y="116" class="feat-label">Endpoint</text>
  <text x="105" y="134" class="feat-label" style="font-size:10px; fill:#8899aa;">(local or API)</text>

  <!-- Feature 2 -->
  <rect x="210" y="35" width="150" height="110" rx="6" class="feat-box"/>
  <text x="285" y="78" class="feat-num">2</text>
  <text x="285" y="98" class="feat-label">Workspace</text>
  <text x="285" y="116" class="feat-label">Integration</text>
  <text x="285" y="134" class="feat-label" style="font-size:10px; fill:#8899aa;">(read/write FS)</text>

  <!-- Feature 3 -->
  <rect x="390" y="35" width="150" height="110" rx="6" class="feat-box"/>
  <text x="465" y="78" class="feat-num">3</text>
  <text x="465" y="98" class="feat-label">App</text>
  <text x="465" y="116" class="feat-label">Runtime</text>
  <text x="465" y="134" class="feat-label" style="font-size:10px; fill:#8899aa;">&nbsp;</text>

  <!-- Arrows between features -->
  <line x1="180" y1="90" x2="210" y2="90" stroke="#4a90d9" stroke-width="1.5" marker-end="url(#arr)"/>
  <line x1="360" y1="90" x2="390" y2="90" stroke="#4a90d9" stroke-width="1.5" marker-end="url(#arr)"/>

  <defs>
    <marker id="arr" markerWidth="8" markerHeight="8" refX="6" refY="3" orient="auto">
      <path d="M0,0 L0,6 L8,3 z" fill="#4a90d9"/>
    </marker>
  </defs>
</svg>

---

## Reference Implementation

One concrete way to implement A2App maps the four features onto three components:

<svg viewBox="0 0 720 420" xmlns="http://www.w3.org/2000/svg" width="720" height="420">
  <defs>
    <style>
      .comp-box    { fill: #161c28; stroke-width: 1.5; rx: 8; }
      .comp-a      { stroke: #4ac994; }
      .comp-b      { stroke: #e8a44a; }
      .comp-c      { stroke: #e84a7a; }
      .comp-title  { font-family: monospace; font-size: 14px; font-weight: bold; }
      .comp-feat   { font-family: monospace; font-size: 11px; fill: #8899bb; }
      .comp-detail { font-family: monospace; font-size: 11px; fill: #aabbcc; }
      .conn-label  { font-family: monospace; font-size: 10px; fill: #778899; text-anchor: middle; }
      .section-label { font-family: monospace; font-size: 10px; fill: #556677; }
    </style>
    <marker id="arrowA" markerWidth="8" markerHeight="8" refX="6" refY="3" orient="auto">
      <path d="M0,0 L0,6 L8,3 z" fill="#4ac994"/>
    </marker>
    <marker id="arrowB" markerWidth="8" markerHeight="8" refX="6" refY="3" orient="auto">
      <path d="M0,0 L0,6 L8,3 z" fill="#e8a44a"/>
    </marker>
    <marker id="arrowC" markerWidth="8" markerHeight="8" refX="6" refY="3" orient="auto">
      <path d="M0,0 L0,6 L8,3 z" fill="#e84a7a"/>
    </marker>
  </defs>

  <!-- Background grid hint -->
  <rect x="0" y="0" width="720" height="420" fill="#0e1318" rx="10"/>

  <!-- ── Component A: Coding Agent ── -->
  <rect x="30" y="30" width="200" height="170" rx="8" class="comp-box comp-a"/>
  <text x="50" y="58" class="comp-title" fill="#4ac994">A. Coding Agent</text>
  <text x="50" y="80" class="comp-feat">covers features 1 + 2</text>
  <line x1="50" y1="90" x2="210" y2="90" stroke="#4ac994" stroke-width="0.5" opacity="0.4"/>
  <text x="50" y="110" class="comp-detail">· LLM inference</text>
  <text x="50" y="128" class="comp-detail">  (local or remote API)</text>
  <text x="50" y="148" class="comp-detail">· Filesystem read/write</text>
  <text x="50" y="168" class="comp-detail">· Code generation + exec</text>

  <!-- ── Component B: Rust Process ── -->
  <rect x="260" y="30" width="200" height="230" rx="8" class="comp-box comp-b"/>
  <text x="280" y="58" class="comp-title" fill="#e8a44a">B. Rust Process</text>
  <text x="280" y="80" class="comp-feat">harness (feature 4)</text>
  <line x1="280" y1="90" x2="440" y2="90" stroke="#e8a44a" stroke-width="0.5" opacity="0.4"/>
  <text x="280" y="110" class="comp-detail">· WS server</text>
  <text x="280" y="128" class="comp-detail">  ← talks to Agent (A)</text>
  <text x="280" y="148" class="comp-detail">· CRDT state</text>
  <text x="280" y="166" class="comp-detail">  ← talks to Runtime (C)</text>
  <text x="280" y="192" class="comp-detail" style="fill:#667788; font-size:10px;">note: agent could run CRDT</text>
  <text x="280" y="208" class="comp-detail" style="fill:#667788; font-size:10px;">but cross-lang issues → WS</text>
  <text x="280" y="224" class="comp-detail" style="fill:#667788; font-size:10px;">conn to agent instead</text>

  <!-- ── Component C: App Runtime ── -->
  <rect x="490" y="30" width="200" height="170" rx="8" class="comp-box comp-c"/>
  <text x="510" y="58" class="comp-title" fill="#e84a7a">C. App Runtime</text>
  <text x="510" y="80" class="comp-feat">runtime (feature 3)</text>
  <line x1="510" y1="90" x2="670" y2="90" stroke="#e84a7a" stroke-width="0.5" opacity="0.4"/>
  <text x="510" y="110" class="comp-detail">· Makepad "host"</text>
  <text x="510" y="128" class="comp-detail">· Launches Splash</text>
  <text x="510" y="146" class="comp-detail">  mini-apps</text>
  <text x="510" y="166" class="comp-detail">· CRDT consumer</text>

  <!-- ══ Connection arrows ══ -->

  <!-- A → B via WebSocket -->
  <line x1="230" y1="115" x2="260" y2="115" stroke="#4ac994" stroke-width="1.8" marker-end="url(#arrowA)"/>
  <line x1="260" y1="125" x2="230" y2="125" stroke="#e8a44a" stroke-width="1.8" marker-end="url(#arrowB)"/>
  <text x="245" y="108" class="conn-label" style="fill:#4ac994">WS</text>

  <!-- B → C via CRDT -->
  <line x1="460" y1="115" x2="490" y2="115" stroke="#e8a44a" stroke-width="1.8" marker-end="url(#arrowB)"/>
  <line x1="490" y1="125" x2="460" y2="125" stroke="#e84a7a" stroke-width="1.8" marker-end="url(#arrowC)"/>
  <text x="475" y="108" class="conn-label" style="fill:#e8a44a">CRDT</text>

  <!-- ══ Feature mapping table ══ -->
  <rect x="30" y="290" width="660" height="110" rx="6" fill="#131920" stroke="#2a3444" stroke-width="1"/>
  <text x="50" y="315" class="comp-title" style="font-size:12px; fill:#778899;">Feature → Component mapping</text>

  <!-- Table rows -->
  <text x="50"  y="338" class="comp-detail" style="fill:#4ac994;">1. Inference endpoint</text>
  <text x="280" y="338" class="comp-detail">→  Component A (coding agent calls LLM)</text>

  <text x="50"  y="358" class="comp-detail" style="fill:#4ac994;">2. Workspace integration</text>
  <text x="280" y="358" class="comp-detail">→  Component A (FS read/write in agent loop)</text>

  <text x="50"  y="378" class="comp-detail" style="fill:#e8a44a;">4. Harness</text>
  <text x="280" y="378" class="comp-detail">→  Component B (Rust process, WS + CRDT hub)</text>

  <text x="50"  y="398" class="comp-detail" style="fill:#e84a7a;">3. App runtime</text>
  <text x="280" y="398" class="comp-detail">→  Component C (Makepad host + Splash mini-apps)</text>
</svg>

---

## Component Notes

**A — Coding Agent** handles both inference (feature 1) and workspace access (feature 2). It runs the LLM loop, reads and writes the filesystem, and communicates outward via a WebSocket connection to the Rust process.

**B — Rust Process** is the harness (feature 4). It runs a WebSocket server that the coding agent connects to, and maintains a CRDT for state synchronisation with the app runtime. The CRDT could in principle live in the coding agent, but cross-language friction made a WebSocket bridge the pragmatic call.

**C — App Runtime** covers the runtime (feature 3). It hosts a Makepad application shell capable of launching Splash mini-apps, and consumes state from the Rust process via the CRDT.
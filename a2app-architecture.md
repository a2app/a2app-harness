# A2App: Agentic App Building and Runtime System

A2App is a system for building and running agentic applications. It composes an inference endpoint, a shared workspace, an app runtime, and a coordination harness into a coherent whole.

---

## Key Features

Four capabilities define the system, independent of any particular implementation:

![A2App key features](a2app-features.svg)

<img src="a2app-features.svg" alt="A2App key features" width="700"/>

---

## Reference Implementation

One concrete way to implement A2App maps the four features onto three components:

![A2App components](a2app-components.svg)

<img src="a2app-components.svg" alt="A2App components" width="720"/>

---

## Component Notes

**A — Coding Agent** handles both inference (feature 1) and workspace access (feature 2). It runs the LLM loop, reads and writes the filesystem, and communicates outward via a WebSocket connection to the Rust process.

**B — Rust Process** is the harness (feature 4). It runs a WebSocket server that the coding agent connects to, and maintains a CRDT for state synchronisation with the app runtime. The CRDT could in principle live in the coding agent, but cross-language friction made a WebSocket bridge the pragmatic call.

**C — App Runtime** covers the runtime (feature 3). It hosts a Makepad application shell capable of launching Splash mini-apps, and consumes state from the Rust process via the CRDT.

# UI/UX & Dynamic Design Specification - AI Upcycler

## 1. Design System & Vision

**AI Upcycler** features a high-aesthetic, modern dark-mode interface built with **Tauri**, **React**, and **CSS**. The visual theme reflects high-tech hardware sustainability, combining deep obsidian backgrounds (`#0B0F19`), vibrant electric teal accents (`#10B981`), and subtle glassmorphic translucent panels.

---

## 2. Color Palette & Typography

```css
:root {
  /* Color Palette */
  --bg-primary: #0b0f19;        /* Obsidian Dark */
  --bg-secondary: #111827;      /* Dark Charcoal Card */
  --bg-glass: rgba(17, 24, 39, 0.75); /* Glassmorphism */
  
  --border-color: rgba(255, 255, 255, 0.1);
  --border-glow: rgba(16, 185, 129, 0.4);

  --accent-green: #10b981;     /* Upcycle Emerald */
  --accent-cyan: #06b6d4;      /* Cyber Cyan */
  --accent-amber: #f59e0b;     /* Warning Amber */
  --accent-danger: #ef4444;    /* Error Red */

  --text-primary: #f9fafb;     /* High contrast white */
  --text-secondary: #9ca3af;   /* Muted gray */

  /* Typography */
  --font-family: 'Inter', system-ui, -apple-system, sans-serif;
  --font-mono: 'JetBrains Mono', monospace;
}
```

---

## 3. Desktop Application Interface (Master App)

The Desktop App consists of four primary views:

```
+-----------------------------------------------------------------------+
|  ♻️ AI Upcycler Master   [Cluster: ACTIVE (16GB)]   [Model: Qwen 14B] |
+-----------------------------------------------------------------------+
| [Dashboard]  [Device Pool]  [Model Library]  [Local Chat]  [Settings] |
+-----------------------------------------------------------------------+
|                                                                       |
|   +--------------------------+   +--------------------------------+   |
|   | Cluster Topology         |   | Real-Time Generation Speed     |   |
|   |                          |   |                                |   |
|   | 🟢 Host Laptop (8GB)     |   | ⚡ 8.4 Tokens/Sec              |   |
|   | 📱 Galaxy S10 (4GB USB)  |   | 📊 68% Pooled RAM Used         |   |
|   | 📱 Pixel 4a (4GB Wi-Fi)  |   | 🌡️ Max Temp: 41°C (Pixel 4a)   |   |
|   +--------------------------+   +--------------------------------+   |
|                                                                       |
|   +---------------------------------------------------------------+   |
|   | Interactive Model Slicing Visualizer                           |   |
|   | [ Master (0-16) ] === [ Phone 1 (17-32) ] === [ Phone 2 (33-48)] |   |
|   +---------------------------------------------------------------+   |
+-----------------------------------------------------------------------+
```

### 3.1 Views & Functionality

1. **Cluster Dashboard**:
   - Live hardware telemetry cards showing CPU/NPU usage, RAM allocation, battery state, and temperatures across all nodes.
   - Interactive pipeline topology visualizer powered by dynamic canvas / CSS micro-animations.
2. **Device Pooling Wizard**:
   - Simple 3-step pairing workflow:
     1. Display QR code on Desktop screen.
     2. Scan QR code using Worker App on mobile phone.
     3. Auto-run benchmark test and add phone to cluster pool.
3. **Model Catalog**:
   - One-click Hugging Face GGUF downloader with quantization filters (Q4_K_M, Q3_K_M, etc.).
4. **Local Chat Workspace**:
   - Low-latency streaming token markdown render engine with syntax highlighting and code copying.

---

## 4. Mobile Worker Agent Interface (Android & iOS)

The mobile agent UI is designed to be lightweight, dark-mode, and low-power, ensuring minimal battery waste.

```
+-----------------------------------+
|  ♻️ AI Upcycler Worker Node       |
+-----------------------------------+
| Status: CONNECTED (USB ADB)      |
| IP: 192.168.1.104:50052           |
+-----------------------------------+
|                                   |
|   RAM Allocated: 3.2 GB / 4.0 GB  |
|   [====================....] 80%  |
|                                   |
|   Assigned Layers: 17 -> 32       |
|   Engine: llama.cpp RPC (ARM64)   |
|                                   |
|   Temperature: 38.5°C 🟢          |
|   Battery: 94% (Charging ⚡)       |
|                                   |
+-----------------------------------+
|  [ Stop Worker ]  [ Device Logs ] |
+-----------------------------------+
```

---

## 5. Micro-Animations & UX Polish

- **Active Compute Glow**: Pulsing CSS radial gradient around active worker cards during forward inference execution.
- **Node Connection Transition**: Smooth spring-animated node entry into the cluster canvas upon successful handshake.
- **Hardware Telemetry Gauges**: Real-time smooth SVG gauge transitions updated at 10 Hz via WebSocket telemetry.

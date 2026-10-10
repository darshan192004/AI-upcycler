# Code Style & Engineering Standards - AI Upcycler

## 1. Overview & General Principles

**AI Upcycler** is a multi-language monorepo codebase spanning **Rust** (Master Orchestrator), **TypeScript / React** (Desktop GUI), **C++20** (Embedded `llama.cpp` RPC Engine), and **Kotlin** (Android Worker Agent).

### Core Principles
1. **Zero Memory Leaks**: Native components (C++/Rust) must enforce RAII memory management.
2. **Explicit Error Handling**: Avoid unhandled panics or silent crashes (`unwrap()` in Rust is forbidden in production code).
3. **Immutability by Default**: Declare variables immutable unless mutation is strictly necessary.
4. **Strict Typing**: TypeScript `any` type is prohibited. Rust strict compiler flags enabled.

---

## 2. Git Branching & Pull Request (PR) Policy

> [!CAUTION]
> **DIRECT PUSHES TO THE `main` BRANCH ARE STRICTLY FORBIDDEN.**
> All code changes, bug fixes, and feature additions MUST follow the PR workflow below.

### 2.1 Branch Naming Conventions
- Features: `feature/<short-description>` (e.g. `feature/mmap-eviction-engine`)
- Bug Fixes: `fix/<short-description>` (e.g. `fix/android-rpc-port-conflict`)
- Documentation: `docs/<short-description>` (e.g. `docs/update-api-schemas`)
- CI/CD: `ci/<short-description>` (e.g. `ci/add-pr-agent-bot`)

### 2.2 Pull Request Lifecycle & AI Review Bot
1. **Create Branch**: Developers/Agents create a dedicated feature branch off `main`.
2. **Push Branch & Open PR**: Push branch to GitHub and create a Pull Request against `main`.
3. **Automated AI Review**: The automated **PR Agent** workflow (`.github/workflows/pr-agent.yml`) triggers on every PR event. The AI reviewer analyzes code diffs, security risks, performance impacts, and code style compliance.
4. **Automated Dependabot Security**: Dependabot (`.github/dependabot.yml`) manages dependency vulnerability updates across Cargo, npm, Gradle, and GitHub Actions.
5. **Review & Approval**: Address any AI PR agent recommendations. Once all status checks pass and code review is approved, merge the PR into `main`.
6. **Automated Release Trigger**: Merging into `main` automatically triggers `.github/workflows/release.yml` to compile and publish new Windows `.exe` desktop installer binaries and Android `.apk` worker assets.

---

## 3. Language-Specific Guidelines

### 3.1 Rust (Master Orchestrator Core)

- **Style Guide**: Follow official `rustfmt` formatting (`cargo fmt`).
- **Linter**: Zero warnings policy on `cargo clippy --all-targets -- -D warnings`.
- **Error Handling**: Use `thiserror` for library error definitions and `anyhow` for binary application entry points.

```rust
// GOOD: Explicit error handling with custom Result types
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SchedulerError {
    #[error("Node {0} has insufficient memory budget: required {1} MB, available {2} MB")]
    InsufficientMemory(String, u64, u64),
    #[error("Network RPC timeout to node {0} after {1} ms")]
    RpcTimeout(String, u64),
}

pub type Result<T> = std::result::Result<T, SchedulerError>;
```

### 3.2 TypeScript & React (Desktop GUI)

- **Style Guide**: ESLint with `prettier` configuration.
- **Component Pattern**: Use functional components with explicit TypeScript interfaces for props.
- **State Management**: Use Zustand stores for global application state.

```tsx
// GOOD: Strongly typed React functional component with explicit props
import React from 'react';

interface NodeCardProps {
  nodeId: string;
  deviceName: string;
  ramUsagePct: number;
  temperatureCelsius: number;
  isConnected: boolean;
  onSelectNode: (id: string) => void;
}

export const NodeCard: React.FC<NodeCardProps> = ({
  nodeId,
  deviceName,
  ramUsagePct,
  temperatureCelsius,
  isConnected,
  onSelectNode,
}) => {
  return (
    <div 
      className={`p-4 rounded-xl border transition-all ${
        isConnected ? 'border-emerald-500/30 bg-gray-900/80' : 'border-gray-800 bg-gray-950'
      }`}
      onClick={() => onSelectNode(nodeId)}
    >
      <h3 className="font-semibold text-gray-100">{deviceName}</h3>
      <p className="text-sm text-gray-400">RAM: {ramUsagePct}%</p>
      <span className="text-xs text-emerald-400">{temperatureCelsius}°C</span>
    </div>
  );
};
```

### 3.3 C++20 (Embedded `llama.cpp` RPC Engine)

- **Standard**: C++20 standard (`-std=c++20`).
- **Formatting**: `clang-format` using Google or LLVM style.
- **Pointer Safety**: Use `std::unique_ptr` and `std::shared_ptr`. Naked raw `new`/`delete` calls are prohibited.

---

## 4. Git Commit Message Conventions

Commit messages must follow the [Conventional Commits](https://www.conventionalcommits.org/) specification:

- `feat(scheduler)`: Add bandwidth-aware layer dynamic programming allocator
- `fix(mmap)`: Fix page eviction bug on Windows file handles
- `docs(api)`: Update REST API OpenAPI specification
- `ci(github)`: Add PR Agent and Dependabot workflows

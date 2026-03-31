## Baryon Architecture

### 1. Directory Structure
```text
src/
├── core/           # Common types and primitives
├── ecs/            # Layer 0: Storage & Concurrency boundaries
├── svp/            # Layer 1: Physical I/O (DMA, io_uring)
├── uast/           # Layer 2: Logical Topology & Metrics
├── engine/         # Layer 3: Background Orchestration
├── ui/             # Layer 4: TUI & Rendering
├── app.rs          # Application lifecycle
└── main.rs         # Entry point
```

### 2. Pillar Assignments

#### Pillar 1: ECS (`src/ecs/`)
- **NodeId**: Move to `src/ecs/id.rs`.
- **UastRegistry**: Move to `src/ecs/registry.rs`.
- **RegistryChunk**: Move to `src/ecs/chunk.rs`.
- *Why*: This is the raw "Storage Layer". It shouldn't know about disks or trees.

#### Pillar 2: SVP (`src/svp/`)
- **SvpPointer**: Move to `src/svp/pointer.rs`.
- **SvpResolver**: Move to `src/svp/resolver.rs`.
- **Ingestion**: Move `ingest_svp_file` to `src/svp/ingest.rs`.
- *Why*: This is the "Physical Layer". It maps hardware blocks to ECS indices.

#### Pillar 3: UAST (`src/uast/`)
- **Topology**: Move `TreeEdges` and LCRS logic to `src/uast/topology.rs`.
- **Metrics**: Move `SpanMetrics` and `metrics_inflated` logic to `src/uast/metrics.rs`.
- **Viewport**: Move `query_viewport`, `RenderToken`, and `Viewport` to `src/uast/projection.rs`.
- **Semantics**: Move `SemanticKind` to `src/uast/kind.rs`.
- *Why*: This is the "Logical Layer". It interprets the ECS indices as a structured tree.

#### Pillar 4: Orchestration (`src/engine/` & `src/app.rs`)
- **Engine**: Encapsulate the background thread and `EditorCommand` into an `Engine` struct.
- **UI**: Encapsulate Ratatui logic, Gutter calculation, and Input handling into a `Frontend` struct.
- **App**: Create a top-level `App` struct that initializes the channels and pillars.

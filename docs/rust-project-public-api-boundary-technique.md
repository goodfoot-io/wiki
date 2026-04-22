# Rust Project Public API Boundary & TDD Initialization Technique

## Purpose

This document outlines a standardized technique for bootstrapping a new Rust project (often within a larger workspace or NPM monorepo) from a detailed architectural specification into a concrete Test-Driven Development (TDD) foundation. 

By defining the public API boundaries (data structures, function signatures) and behavioral expectations (skipped integration tests) upfront, we create a clear, actionable, and verifiable roadmap for development. This approach minimizes "blank canvas" paralysis, aligns the implementation strictly with the specification, and provides a continuous feedback loop as implementation progresses.

### Primary Goal: Getting the Types Right

In Rust, the type system and borrow checker are the architectural backbone of any application. A primary superpower of this technique is **consumer-driven type validation**. 

Because `#[ignore]`-annotated tests are still fully compiled and type-checked by `cargo test` or `cargo check --tests`, writing tests against function stubs guarantees that:
1. **Ownership and Lifetimes are Sound:** The compiler verifies that data is passed, borrowed, and returned correctly across the public API boundary.
2. **Ergonomics are Validated:** If an API requires excessive cloning, awkward lifetime annotations, or impossible trait bounds, the developer discovers this immediately when trying to write the test, rather than after sinking hours into internal implementation logic.
3. **Data Structures are Complete:** Writing tests forces the developer to add necessary standard derives (e.g., `Clone`, `Debug`, `PartialEq`) and design a distinct error boundary (e.g., `Result<T, E>`).

By the time the stubs and ignored tests compile together, you have mathematical proof from the Rust compiler that your public API boundary is structurally viable.

## Target Audience

This guide is intended for AI agents (like Gemini) and human developers who are tasked with initiating a new Rust codebase based on a comprehensive design document.

## The Technique

When presented with a new architectural specification and asked to lay the groundwork for implementation, follow these phases:

### Phase 1: Analyze the Specification

Thoroughly read the design document to extract the core components of the system:
1.  **Domain Models & State**: What are the core entities? Are they mutable or immutable? How are they stored?
2.  **Input/Output (DTOs)**: What shapes of data do the primary operations accept and return?
3.  **Operations**: What are the main actions the system performs? What are their success and failure conditions?
4.  **Invariants & Business Logic**: What rules must always hold true?

### Phase 2: Draft the Implementation Plan

Before writing code, create a planning document (e.g., `docs/<project-name>-test-creation-plan.md`). This plan should explicitly state the "Commander's Intent," giving implementers the flexibility to adjust Rust-specific types (to satisfy the borrow checker or improve ergonomics) as long as the core semantics of the specification are preserved.

The plan must outline:
- The exact Data Structures to be created.
- The Function Stubs to be defined.
- The specific Integration Tests to be written, categorized by feature.

### Phase 3: Define Data Structures & Types

Create the core data structures in the Rust source code (e.g., `src/types.rs` or relevant modules).
- Translate the conceptual models from the spec into concrete Rust `struct`s and `enum`s.
- Derive necessary standard traits (`Clone`, `Debug`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`).
- Do not add implementation logic or helper methods at this stage unless they are strictly defining the API boundary.

### Phase 4: Create Function Stubs

Define the public API boundaries by creating function stubs for all primary operations described in the spec.
- Ensure function signatures have correct argument types and return values (e.g., heavily utilizing `Result<T, E>`).
- **Do not write implementation logic.**
- Use `todo!()` or return generic errors (e.g., `Err(anyhow::anyhow!("Not implemented"))`) for the function bodies.
- This ensures the project compiles and establishes the exact interface the tests will call.

### Phase 5: Write Skipped Integration Tests

Create a comprehensive suite of integration tests (e.g., in the `tests/` directory). These tests are the executable form of the specification.
- Write tests that perform actual setup (e.g., initializing a dummy environment or workspace), call the function stubs, and assert the expected outcomes or errors.
- **Crucial Step:** Annotate *every single test* with `#[ignore]`.
- Because the tests are ignored, the `cargo test` suite will compile and "pass" immediately. This proves the API boundaries are structurally sound and type-safe.

## Execution and Iteration

Once the Data Structures, Function Stubs, and Skipped Tests are in place, the project is ready for the implementation phase. 

The development workflow becomes a standard TDD loop:
1.  Remove the `#[ignore]` attribute from one test.
2.  Run `cargo test`. The test will fail (hitting a `todo!()` or "Not implemented" error).
3.  Write the minimal implementation in the function stub to make the test pass.
4.  Refactor as needed.
5.  Repeat until all tests are unskipped and passing.

## Why this works well for Rust

Rust's strict type system and borrow checker mean that API design choices have deep architectural consequences. By forcing the definition of types, function signatures, and consumer usage (via tests) before implementation begins, you discover borrow-checker constraints and ergonomic issues early, avoiding massive refactoring later.
# JevCode

<!-- impeccable:product-schema 1 -->

## Platform

web

Desktop application using a Tauri window, with React rendering the interface.

## Stack

User specified Tauri 2, React, TypeScript, Vite, Rust, Tailwind CSS, SQLite,
OS keychain or Stronghold credentials, and Git CLI or git2-rs.

## Product Purpose

A desktop AI coding agent inspired by the interaction model of ChatGPT Codex
Desktop and Claude Code. The immediate deliverable is a modular, runnable
foundation with shared provider contracts and documented architecture.

## Operating Context

The user selected a project sidebar and central agent conversation. Project
selection, provider configuration, permissions and usage must be accessible.

## Capabilities and Constraints

Provider logic belongs outside the UI. OpenAI, Anthropic, Gemini, OpenCode Zen
and OpenCode Go share an agent runtime. Local data and credentials use separate
stores. First implementation supports read-only project tools and a labeled
offline preview adapter. Editing, arbitrary shell execution and production
provider validation are future scope.

## Brand Commitments

Name: JevCode. Familiar project-sidebar and conversation interaction was
explicitly selected by the user. The user selected mockup-first design.

## Open Decisions

Assumption: the primary audience is developers working in local repositories.
No specific palette, logo or display type was supplied.

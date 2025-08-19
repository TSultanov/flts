# Copilot Instructions for FLTS Project

This document contains design decisions and architectural guidelines for the FLTS (Foreign Language Translation Study) project.

## Project Overview

FLTS is a language learning application built with Svelte 5, TypeScript, and a client‑side SQLite (WASM) database. It allows users to import books, organize them in folders, and translate text with word-level analysis.

## Architecture Decisions

### Frontend Framework
- **Svelte 5**: Using the latest Svelte version with runes (`$state`, `$derived`, `$effect`, `$props`)
- **TypeScript**: Strict typing throughout the application
- **Vite**: Build tool and development server
- **pnpm**: Package manager for dependency management

### Data Layer
- **SQLite (WASM)**: Persistent structured storage using `@sqlite.org/sqlite-wasm`
- **Abstraction Layer**: All SQL access consolidated in `src/lib/data/sql` (e.g. `book.ts`, `dictionary.ts`)
- **Library Pattern**: Centralized higher-level data access via the `Library` class in `library.svelte.ts`
- **Queue/Cache**: A minimal Dexie usage for translation queue & cache (`queueDb.ts`, `cache.ts`)

### Database Schema
Books are organized in a hierarchical folder structure:
- Books have an optional `path?: string[]` property
- `null` or empty path = root folder
- Path array represents folder hierarchy (e.g., `["Fiction", "Sci-Fi"]`)
- The `getLibraryBooks()` method returns a `LibraryFolder` tree structure

## Design System & Styling

### CSS Variables
All colors and design tokens are defined in `app.css` as CSS variables. Always use these variables instead of hardcoded colors.

### Button System
All buttons use a unified design system with these classes:

#### Button Variants
- **No class (default/primary)**: Dark background, light text
- **`.secondary`**: Light background, dark text
- **`.danger`**: Red background for destructive actions
- **`.compact`**: Smaller padding and font size

#### Button Usage Guidelines
- Primary actions: Use default/primary styling (no additional class)
- Cancel/secondary actions: Use `.secondary` class
- Destructive actions (delete): Use `.danger` class
- Space-constrained areas: Add `.compact` class

### Component Patterns

#### Confirmation Dialogs
- Use `ConfirmDialog.svelte` component for all destructive actions
- HTML5 `<dialog>` element with backdrop
- Bindable `isOpen` prop
- Consistent button styling (`.secondary` for cancel, `.danger` for confirm)

#### Folder Structure Display
- Recursive components using Svelte 5 snippets (`{#snippet}` and `{@render}`)
- HTML `<details>` elements for collapsible folders
- Root folder (no name) has hidden chevron via CSS
- Unified rendering logic for both root and nested folders

## Code Standards

### Svelte 5 Patterns
- Use runes: `$state`, `$derived`, `$effect`, `$props`
- Prefer snippets over components for simple, recursive structures
- Use `{@render snippet()}` for snippet invocation

### TypeScript
- Strict typing enabled
- Interface definitions for all props
- Explicit return types for complex functions

### CSS Guidelines
- Use CSS variables defined in `app.css`
- Avoid hardcoded colors or dimensions
- Component-specific styles in `<style>` blocks
- Global button styles, no component-specific button CSS

### Data Access
- All core content (books, chapters, paragraphs, sentences, words, translations) is stored in SQLite accessed through wrapper modules in `src/lib/data/sql/`
- The `Library` class orchestrates higher-level operations (import, move, delete) without embedding raw SQL
- Translation queue & cache still use Dexie temporarily; avoid adding new Dexie usage—prefer SQLite for new features
- Plan for reactive layer: emit events after write operations; until then consumers should refetch when needed
- Maintain transaction boundaries within SQL helper functions where consistency is required

## Key Implementation Details

### Book Deletion
- Must use confirmation dialog before deletion
- Cascade delete: book → chapters → paragraphs → translations
- Implemented in `library.svelte.ts` `deleteBook()` method

### Import System
- Support for plain text and EPUB files
- Chapter selection for EPUB imports
- Progress indication during import

### Navigation
- File-based routing with `@mateothegreat/svelte5-router`
- Book URLs: `/book/{bookUid}/{chapterUid?}` (using UUIDs)

## Future Development Guidelines

1. **Always use the unified button system** - don't create custom button styles
2. **Follow the folder hierarchy pattern** - use `path` array for organization
3. **Use confirmation dialogs** for all destructive actions
4. **Prefer SQLite for new persistence work** (do not introduce new Dexie stores)
5. **Phase out legacy Dexie usage** (queue/cache) when practical, replacing with SQLite tables & batched queries
6. **Keep styles centralized** in `app.css` using CSS variables
7. **Use Svelte 5 patterns** consistently (runes, snippets)
8. **Type everything** - maintain strict TypeScript compliance
9. **Use pnpm for package management** - run `pnpm install`, `pnpm add`, etc. instead of npm

## Testing Setup & Guidelines

### Test Framework
- **Vitest**: Modern testing framework with jsdom environment
- **fake-indexeddb**: IndexedDB mocking for database tests  
- **fast-check**: Property-based testing for complex data structures
- **Stryker**: Mutation testing for test quality assurance

### Test Configuration
Configuration files: `vitest.config.ts`, `stryker.conf.json`, and test scripts in `package.json`

### Test Commands
Run `pnpm test` (watch), `pnpm test:coverage`, `pnpm test:ui`, or `pnpm test:mutation`

### Testing Patterns
- **SQLite Tests**: Initialize an in-memory / WASM database per test (or per suite) and apply migrations before exercising queries
- **Legacy Dexie Tests**: Where queue/cache still rely on Dexie, continue to mock or use `fake-indexeddb` until migrated
- **Component Tests**: Use jsdom, mock external dependencies, test props/events/reactive state
- **Property-Based Tests**: Use fast-check for folder hierarchies and data transformations

### Coverage & Quality
- Coverage thresholds in `vitest.config.ts` (lines: 7%, functions: 25%, branches: 65%, statements: 7%)
- Mutation testing thresholds: High 80%, Low 60%, Break 50%
- Reports in `coverage/` and `reports/mutation/`

### Best Practices
1. Mock external dependencies (config, APIs, file system)
2. Apply and verify SQLite migrations before tests; reset DB state between tests
3. Exercise transaction-bound operations to ensure atomicity (imports, cascading deletes)
4. Use descriptive test names explaining scenarios
5. Cover both happy paths and error conditions (missing UIDs, constraint violations)
6. Test folder hierarchy operations thoroughly
7. Validate button styling and component consistency
8. Test confirmation dialog workflows for destructive actions
9. Use property-based testing for complex data operations
10. Maintain high mutation test scores for test quality
11. Add tests for new SQLite query helpers (book / translation fetchers) as they are introduced

# Copilot Instructions for FLTS Project

This document contains design decisions and architectural guidelines for the FLTS (Foreign Language Translation Study) project.

## Project Overview

FLTS is a language learning application built with Svelte 5, TypeScript, and Dexie.js (IndexedDB). It allows users to import books, organize them in folders, and translate text with word-level analysis.

## Architecture Decisions

### Frontend Framework
- **Svelte 5**: Using the latest Svelte version with runes (`$state`, `$derived`, `$effect`, `$props`)
- **TypeScript**: Strict typing throughout the application
- **Vite**: Build tool and development server

### Data Layer
- **Dexie.js**: IndexedDB wrapper for client-side data persistence
- **Reactive Queries**: Using `liveQuery` from Dexie for reactive data binding
- **Library Pattern**: Centralized data access through `Library` class in `library.svelte.ts`

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
- All database operations go through the `Library` class
- Use reactive queries with `liveQuery` for real-time updates
- Transaction-based operations for data consistency

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
- Book URLs: `/book/{bookId}/{chapterId?}`

## Future Development Guidelines

1. **Always use the unified button system** - don't create custom button styles
2. **Follow the folder hierarchy pattern** - use `path` array for organization
3. **Use confirmation dialogs** for all destructive actions
4. **Maintain reactive data patterns** with `liveQuery`
5. **Keep styles centralized** in `app.css` using CSS variables
6. **Use Svelte 5 patterns** consistently (runes, snippets)
7. **Type everything** - maintain strict TypeScript compliance

## Testing Considerations
- Database operations should be wrapped in transactions
- Test folder hierarchy creation and navigation
- Verify button styling consistency across components
- Test confirmation dialog workflows

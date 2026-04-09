# Write or update README

Write or update the README.md file for this project.
Use the CLAUDE.md file and the actual source code as the source of truth.

## Writing rules

Follow these rules strictly when writing README text:

- Use English language
- Simple, clear language (understandable by a 10-year-old). Short sentences
- Focus on solving the reader's problem, not listing features
- Consistent terminology. Practical examples for code-related content
- Avoid marketing buzzwords: "seamlessly integrated", "cutting-edge", "game-changer", "let's dive in", "streamline your workflow", "scalable solution", etc. Use plain, specific language instead
- Use regular hyphen `-` for dashes, not em dash `—` or en dash `–`
- Use `→` for arrows

## Process

1. Read CLAUDE.md for project context and architecture
2. Read the current source code to verify what is actually implemented
3. Compare with the existing README.md (if any) to see what changed
4. Write or update README.md so it matches the current state of the code
5. Do not describe planned features as if they already work. Separate "what works now" from "what is not done yet"
6. Include practical curl/shell examples that match the real API
7. Show real config examples that match the current model structs

## What to include

- One-line project description
- Current status (what works, what does not)
- Prerequisites
- Quick start with numbered steps
- Config file examples with field explanations
- HTTP API endpoints with request/response examples
- On-disk file layout
- Notes about important behaviors
- Development commands (build, test, run)

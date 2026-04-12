# Tasks App — Design Document

## Overview

A built-in project management feature for Uncloud combining Kanban board visualization with Todoist-style scheduled task lists. Supports collaboration via per-project sharing, recurring tasks, subtasks, file attachments, and real-time sync across clients via SSE.

## Core Concepts

### Hierarchy

```
Project → Section → Task → Subtask (recursive)
```

- **Project**: top-level container. Owned by a user, optionally shared with others. Has a color and icon for visual distinction.
- **Section**: groups tasks within a project (e.g., "Frontend", "Backend", "Design"). Determines vertical grouping in list view; in board view, all tasks in a project share the same status columns regardless of section.
- **Task**: the atomic work unit. Has status, priority, due date, assignee, labels, description.
- **Subtask**: a task whose `parent_task_id` points to another task. Subtasks appear as a checklist inside their parent card on the board, and as indented children in list view. Subtasks have their own status but do NOT appear as independent board cards.

### Status Model

Fixed set (not user-configurable):

| Status | Display | Board Column | Notes |
|--------|---------|--------------|-------|
| `todo` | To Do | Backlog | Default for new tasks |
| `in_progress` | In Progress | In Progress | Optional `status_note` |
| `blocked` | Blocked | Blocked | Optional `status_note` (reason) |
| `done` | Done | Done | Sets `completed_at` timestamp |
| `cancelled` | Cancelled | Hidden | Optional `status_note`; filtered out by default |

The `status_note` field is a short free-text annotation displayed on the card (e.g., "waiting for delivery", "need Alice's input"). Cleared automatically when status changes.

### Priority

Three levels: **High**, **Medium**, **Low** (default: none/unset).

Displayed as colored indicators:
- High: red/urgent
- Medium: yellow/amber  
- Low: blue/muted

### Views

Three views over the same underlying data:

1. **Board View** (per-project)
   - Columns: Backlog | In Progress | Blocked | Done
   - Cards: tasks (top-level only; subtasks shown as progress within card)
   - Drag-and-drop between columns changes status
   - Drag-and-drop within a column reorders
   - Sections shown as swimlanes (optional toggle)
   - Card shows: title, priority indicator, assignee avatar, due date, subtask progress (e.g., "2/4"), label chips, attachment icon

2. **List View** (per-project)
   - Tree structure: Section → Task → Subtask (collapsible)
   - Inline status chip, priority dot, due date, assignee
   - Drag-and-drop to reorder within section
   - Bulk actions on selected tasks

3. **Schedule View** (cross-project)
   - Groups: Overdue | Today | Tomorrow | Next 7 Days | Later
   - Shows tasks from all projects the user has access to
   - Recurring task instances appear at their computed dates
   - Quick-complete without navigating to the project

### Recurring Tasks

A task can have a `recurrence_rule` defining repetition:

```rust
enum RecurrenceRule {
    Daily,
    Weekly { days: Vec<Weekday> },      // e.g., every Mon/Wed/Fri
    Monthly { day_of_month: u8 },        // e.g., 1st of each month
    Yearly { month: u8, day: u8 },       // e.g., every March 15
    Custom { interval_days: u32 },       // e.g., every 14 days
}
```

**Behavior on completion:**
- When a recurring task is marked `done`, the system creates a new task instance with:
  - Same title, description, section, labels, priority, assignee, recurrence rule
  - `due_date` = next occurrence computed from the rule
  - Fresh status (`todo`), no subtasks (subtasks don't carry over)
- The completed instance stays in history (visible in "Completed" filter)
- Skipping (marking cancelled) does NOT generate the next instance

### Subtasks

- A subtask is a `Task` with `parent_task_id` set
- Maximum nesting depth: 1 level (subtasks cannot have sub-subtasks). This keeps the model simple and avoids JIRA-style complexity.
- Subtasks have their own status, assignee, due date, priority
- On the board, parent card shows: "3/5 subtasks done" progress bar
- Completing all subtasks does NOT auto-complete the parent (explicit action required)
- A subtask can be "promoted" to a top-level task (clears `parent_task_id`, moves to same section)

### Labels

- Per-project label set
- Each label has a `name` and `color`
- A task can have multiple labels
- Labels appear as small colored chips on cards

### File Attachments

- A task can link to one or more Uncloud files via `TaskAttachment`
- Displayed as clickable file chips (thumbnail for images, icon+name for others)
- Attaching a file does NOT move/copy it — it's a reference
- If the linked file is deleted (trashed), the attachment shows as "file removed"

### Comments

- Markdown-formatted discussion on a task
- Visible in a thread when the task card is opened
- Comment count shown on the card exterior
- No @mentions or reactions in v1 (can add later)

### Collaboration

- A project can be shared with other Uncloud users
- Permission levels: **Viewer** (read-only), **Editor** (full CRUD on tasks), **Admin** (can share/unshare, delete project)
- Project owner is implicitly Admin
- Assignee must be a member of the project
- SSE events propagate all mutations to other clients viewing the same project

---

## Data Model

### MongoDB Collections

#### `task_projects`

```rust
struct TaskProject {
    id: ObjectId,
    name: String,
    description: Option<String>,
    color: String,                          // hex color, e.g., "#3B82F6"
    icon: Option<String>,                   // emoji or icon name
    owner_id: ObjectId,
    members: Vec<ProjectMember>,
    default_view: ProjectView,              // Board or List
    archived: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

struct ProjectMember {
    user_id: ObjectId,
    username: String,                       // denormalized for display
    permission: ProjectPermission,          // Viewer, Editor, Admin
    added_at: DateTime<Utc>,
}

enum ProjectPermission {
    Viewer,
    Editor,
    Admin,
}

enum ProjectView {
    Board,
    List,
}
```

#### `task_sections`

```rust
struct TaskSection {
    id: ObjectId,
    project_id: ObjectId,
    name: String,
    position: i32,                          // sort order within project
    collapsed: bool,                        // UI hint for list view
    created_at: DateTime<Utc>,
}
```

#### `tasks`

```rust
struct Task {
    id: ObjectId,
    project_id: ObjectId,
    section_id: Option<ObjectId>,           // None = "unsectioned"
    parent_task_id: Option<ObjectId>,       // None = top-level task
    title: String,
    description: Option<String>,            // Markdown
    status: TaskStatus,
    status_note: Option<String>,            // short annotation
    priority: Option<TaskPriority>,         // None = unset
    assignee_id: Option<ObjectId>,
    labels: Vec<String>,                    // label names (denormalized)
    due_date: Option<NaiveDate>,            // date only, no time
    recurrence_rule: Option<RecurrenceRule>,
    position: i32,                          // sort within section+status
    attachments: Vec<ObjectId>,             // file IDs
    created_by: ObjectId,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
}

enum TaskStatus {
    Todo,
    InProgress,
    Blocked,
    Done,
    Cancelled,
}

enum TaskPriority {
    High,
    Medium,
    Low,
}

enum RecurrenceRule {
    Daily,
    Weekly { days: Vec<u8> },               // 0=Mon, 6=Sun
    Monthly { day_of_month: u8 },
    Yearly { month: u8, day: u8 },
    Custom { interval_days: u32 },
}
```

#### `task_comments`

```rust
struct TaskComment {
    id: ObjectId,
    task_id: ObjectId,
    project_id: ObjectId,                   // for access control queries
    author_id: ObjectId,
    body: String,                           // Markdown
    created_at: DateTime<Utc>,
    updated_at: Option<DateTime<Utc>>,      // if edited
}
```

#### `task_labels`

```rust
struct TaskLabel {
    id: ObjectId,
    project_id: ObjectId,
    name: String,
    color: String,                          // hex
}
```

### Indexes

```
task_projects: { owner_id: 1 }, { "members.user_id": 1 }
task_sections: { project_id: 1, position: 1 }
tasks: { project_id: 1, status: 1, position: 1 }
tasks: { project_id: 1, section_id: 1, position: 1 }
tasks: { parent_task_id: 1 }
tasks: { assignee_id: 1, due_date: 1 }          // for schedule view
tasks: { due_date: 1, status: 1 }               // for schedule view
task_comments: { task_id: 1, created_at: 1 }
task_labels: { project_id: 1 }
```

---

## API Routes

All routes under `/api/tasks/` require authentication.

### Projects

| Method | Route | Purpose |
|--------|-------|---------|
| GET | `/api/tasks/projects` | List user's projects (owned + member of) |
| POST | `/api/tasks/projects` | Create project |
| GET | `/api/tasks/projects/{id}` | Get project details |
| PUT | `/api/tasks/projects/{id}` | Update project (name, color, icon, default_view) |
| DELETE | `/api/tasks/projects/{id}` | Delete project (owner/admin only) |
| POST | `/api/tasks/projects/{id}/members` | Add member |
| PUT | `/api/tasks/projects/{id}/members/{user_id}` | Update member permission |
| DELETE | `/api/tasks/projects/{id}/members/{user_id}` | Remove member |

### Sections

| Method | Route | Purpose |
|--------|-------|---------|
| GET | `/api/tasks/projects/{id}/sections` | List sections |
| POST | `/api/tasks/projects/{id}/sections` | Create section |
| PUT | `/api/tasks/sections/{id}` | Update section (name, position) |
| DELETE | `/api/tasks/sections/{id}` | Delete section (moves tasks to unsectioned) |
| PUT | `/api/tasks/projects/{id}/sections/reorder` | Reorder sections |

### Tasks

| Method | Route | Purpose |
|--------|-------|---------|
| GET | `/api/tasks/projects/{id}/tasks` | List tasks (query: status, section_id, assignee) |
| POST | `/api/tasks/projects/{id}/tasks` | Create task |
| GET | `/api/tasks/{id}` | Get task details (includes subtasks, comments count) |
| PUT | `/api/tasks/{id}` | Update task fields |
| DELETE | `/api/tasks/{id}` | Delete task (and subtasks) |
| PUT | `/api/tasks/{id}/status` | Change status (+ optional status_note) |
| PUT | `/api/tasks/{id}/position` | Reorder within column/section |
| POST | `/api/tasks/{id}/subtasks` | Create subtask |
| POST | `/api/tasks/{id}/promote` | Promote subtask to top-level task |
| POST | `/api/tasks/{id}/attachments` | Attach file(s) |
| DELETE | `/api/tasks/{id}/attachments/{file_id}` | Remove attachment |

### Schedule (cross-project)

| Method | Route | Purpose |
|--------|-------|---------|
| GET | `/api/tasks/schedule` | Tasks grouped by date (overdue, today, upcoming) |
| GET | `/api/tasks/assigned-to-me` | All tasks assigned to current user |

### Comments

| Method | Route | Purpose |
|--------|-------|---------|
| GET | `/api/tasks/{id}/comments` | List comments on a task |
| POST | `/api/tasks/{id}/comments` | Add comment |
| PUT | `/api/tasks/comments/{id}` | Edit comment (author only) |
| DELETE | `/api/tasks/comments/{id}` | Delete comment (author or admin) |

### Labels

| Method | Route | Purpose |
|--------|-------|---------|
| GET | `/api/tasks/projects/{id}/labels` | List project labels |
| POST | `/api/tasks/projects/{id}/labels` | Create label |
| PUT | `/api/tasks/labels/{id}` | Update label |
| DELETE | `/api/tasks/labels/{id}` | Delete label (removes from tasks) |

---

## SSE Events

New `ServerEvent` variants for real-time sync:

```rust
// Broadcast to all project members
TaskCreated { project_id, task }
TaskUpdated { project_id, task_id, changes }
TaskDeleted { project_id, task_id }
TaskMoved { project_id, task_id, from_status, to_status }

SectionCreated { project_id, section }
SectionUpdated { project_id, section_id }
SectionDeleted { project_id, section_id }

CommentAdded { project_id, task_id, comment }

ProjectMemberAdded { project_id, user_id }
ProjectMemberRemoved { project_id, user_id }
```

The `EventService` already supports per-user channels. For task events, broadcast to all `members[].user_id` of the project.

---

## Frontend

### Routes

```rust
#[derive(Routable)]
enum Route {
    // ... existing routes ...
    #[route("/tasks")]
    TaskSchedule {},                              // Schedule view (default)
    #[route("/tasks/project/:id")]
    TaskProject { id: String },                   // Board or List view
    #[route("/tasks/project/:id/settings")]
    TaskProjectSettings { id: String },           // Project settings + sharing
}
```

### Sidebar

New "Tasks" section in the sidebar:
- "Schedule" link (today/upcoming)
- List of projects (with color dot), clicking opens board/list
- "+ New Project" button at bottom

### Components

```
components/tasks/
  mod.rs                    — Tasks page shell + routing
  schedule_view.rs          — Cross-project schedule (Today/Upcoming/Overdue)
  board_view.rs             — Kanban board with drag-and-drop columns
  board_card.rs             — Individual task card on the board
  list_view.rs              — Tree-structured task list
  task_detail.rs            — Task detail panel/modal (description, subtasks, comments, attachments)
  task_form.rs              — Create/edit task form (shared between quick-add and detail)
  section_header.rs         — Section name + collapse toggle
  comment_thread.rs         — Comment list + add comment form
  project_settings.rs       — Project settings (name, color, members, labels)
  label_picker.rs           — Label selection dropdown (create inline)
  recurrence_picker.rs      — Recurrence rule selector
```

### Board Drag-and-Drop

Reuse the pointer-event pattern from playlist reordering:
- `onpointerdown` on card initiates drag
- Columns have `onpointerenter` to detect target column (status change)
- Within-column: rows have `onpointerenter` for position tracking
- `onpointerup` commits the move (optimistic UI + API call)
- Visual: dragged card at reduced opacity, drop target highlighted

### Task Detail Panel

Opens as a slide-over panel (not a full page navigation) so board context is preserved:
- Title (editable inline)
- Status selector + status note input
- Priority, assignee, due date, labels (all editable)
- Description (Markdown editor, rendered on blur)
- Subtasks list (add/complete/delete)
- File attachments (link picker from Uncloud files)
- Comments thread at the bottom
- Activity log (status changes, assignments) — stretch goal

---

## Implementation Plan

### Phase 1: Core Data Model + API (backend)

1. Add types to `uncloud-common`:
   - `TaskProject`, `TaskSection`, `Task`, `TaskComment`, `TaskLabel` response types
   - Request types for create/update operations
   - `TaskStatus`, `TaskPriority`, `RecurrenceRule`, `ProjectPermission` enums

2. Add models to `uncloud-server`:
   - MongoDB models in `models/task.rs`
   - Collection indexes in `db.rs`

3. Implement routes in `routes/tasks.rs`:
   - Project CRUD + member management
   - Section CRUD + reorder
   - Task CRUD + status change + reorder
   - Subtask creation + promotion
   - Schedule endpoint (cross-project query with date grouping)
   - Comment CRUD
   - Label CRUD
   - Attachment link/unlink

4. Access control middleware:
   - Check project membership on all task operations
   - Enforce permission levels (Viewer can't edit, only Admin can share)

5. Recurring task logic:
   - On status change to `done`: if `recurrence_rule` is set, create next instance
   - `compute_next_due_date(rule, current_due)` helper

6. SSE events:
   - Emit task/section/comment events to all project members

### Phase 2: Frontend — Board View

1. Add "Tasks" section to sidebar + router
2. Project list in sidebar
3. Board view component:
   - Columns rendered from status enum
   - Cards fetched per-project
   - Drag-and-drop between columns (status change) and within (reorder)
4. Task detail slide-over panel:
   - All fields editable
   - Subtask list
   - Comment thread
5. Quick-add: "+" button at top of each column, inline title input

### Phase 3: Frontend — List View + Schedule

1. List view with tree structure (section → task → subtask)
2. Inline editing (click to edit title, click status chip to cycle)
3. Schedule view (cross-project, grouped by date)
4. Recurring task UI (recurrence picker in task detail)

### Phase 4: Polish

1. File attachment picker (browse Uncloud files)
2. Label management UI
3. Project settings + member management
4. Keyboard shortcuts (n = new task, Enter = open, Escape = close)
5. Empty states, loading skeletons
6. Mobile responsiveness (board scrolls horizontally, list is primary on mobile)

---

## Design Decisions & Rationale

| Decision | Rationale |
|----------|-----------|
| Fixed statuses (not custom columns) | Keeps board consistent across projects; avoids configuration overhead for personal use. 5 statuses cover real workflows without JIRA bloat. |
| Subtasks max 1 level deep | Prevents complexity explosion. If you need deeper nesting, break into separate tasks. |
| `status_note` as a first-class field | Quick context ("waiting for X") without opening a comment thread. Visible on card. |
| Per-project sharing (not per-task) | Simpler mental model. You share a project, not individual items. Matches how real collaboration works. |
| Board and List as views, not modes | Same data, different visualization. Switch freely. No data migration between "board projects" and "list projects". |
| Labels per-project | Prevents namespace pollution. Each project has its own vocabulary. |
| File attachments as references | No copying/moving files. A task points to a file. If file is trashed, attachment shows broken state. |
| NaiveDate for due dates | No timezone confusion for dates. "Due April 15" means April 15 in the user's perception, not a UTC timestamp. |
| `position: i32` for ordering | Simple integer positions. On reorder, update affected positions. If gaps get too large, compact (rare operation). |
| Built-in, not sidecar | Visual consistency, shared auth/sessions, same DB, no proxy overhead. Tasks are core to personal cloud use. |

---

## Open Questions (future iterations)

- **Templates**: pre-built project templates (e.g., "Home Renovation", "Trip Planning")?
- **Filters/saved views**: custom filters beyond the three built-in views?
- **Time tracking**: log time spent on tasks?
- **Calendar integration**: show due dates on a calendar view?
- **Email-to-task**: create tasks via email forwarding?
- **Mobile quick-add**: widget or shortcut for instant task capture?

import { Editor } from "@tiptap/core";
import Link from "@tiptap/extension-link";
import Placeholder from "@tiptap/extension-placeholder";
import Underline from "@tiptap/extension-underline";
import StarterKit from "@tiptap/starter-kit";

const editors = new Map();

function normalizedHtml(editor) {
  const html = editor.getHTML().trim();
  return html === "<p></p>" ? "" : html;
}

function dispatchState(id, editor) {
  window.dispatchEvent(new CustomEvent("uncloud:mail-editor-change", {
    detail: {
      id,
      html: normalizedHtml(editor),
      text: editor.getText({ blockSeparator: "\n\n" }).trim(),
    },
  }));
}

function destroy(id) {
  const current = editors.get(id);
  if (!current) {
    return;
  }
  current.destroy();
  editors.delete(id);
}

function mount(id, content) {
  const element = document.getElementById(id);
  if (!element) {
    return false;
  }

  destroy(id);

  const editor = new Editor({
    element,
    extensions: [
      StarterKit.configure({
        heading: false,
        horizontalRule: false,
        code: false,
        codeBlock: false,
      }),
      Underline,
      Link.configure({
        autolink: true,
        linkOnPaste: true,
        openOnClick: false,
        protocols: ["mailto"],
      }),
      Placeholder.configure({
        placeholder: "Write your message...",
      }),
    ],
    content: content && content.trim() ? content : "<p></p>",
    editorProps: {
      attributes: {
        class: "uc-mail-editor-prose",
      },
    },
    onUpdate: ({ editor }) => dispatchState(id, editor),
    onSelectionUpdate: ({ editor }) => dispatchState(id, editor),
    onCreate: ({ editor }) => dispatchState(id, editor),
  });

  editors.set(id, editor);
  return true;
}

function setLink(editor) {
  const current = editor.getAttributes("link").href || "";
  const next = window.prompt("Link URL", current);
  if (next === null) {
    return;
  }
  const value = next.trim();
  if (!value) {
    editor.chain().focus().extendMarkRange("link").unsetLink().run();
    return;
  }
  editor.chain().focus().extendMarkRange("link").setLink({ href: value }).run();
}

function command(id, action) {
  const editor = editors.get(id);
  if (!editor) {
    return false;
  }

  switch (action) {
    case "bold":
      editor.chain().focus().toggleBold().run();
      break;
    case "italic":
      editor.chain().focus().toggleItalic().run();
      break;
    case "underline":
      editor.chain().focus().toggleUnderline().run();
      break;
    case "bulletList":
      editor.chain().focus().toggleBulletList().run();
      break;
    case "orderedList":
      editor.chain().focus().toggleOrderedList().run();
      break;
    case "blockquote":
      editor.chain().focus().toggleBlockquote().run();
      break;
    case "link":
      setLink(editor);
      break;
    case "undo":
      editor.chain().focus().undo().run();
      break;
    case "redo":
      editor.chain().focus().redo().run();
      break;
    default:
      return false;
  }

  dispatchState(id, editor);
  return true;
}

window.UncloudMailEditor = {
  mount,
  destroy,
  command,
};

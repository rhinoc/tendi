import StarterKit from "@tiptap/starter-kit";
import { Markdown } from "@tiptap/markdown";
import { Table as TiptapTable, TableCell, TableHeader, TableRow } from "@tiptap/extension-table";
import TaskList from "@tiptap/extension-task-list";
import TaskItem from "@tiptap/extension-task-item";

import { PromptXmlDecorations } from "./prompts.ts";

export const tiptapExtensions = [
  StarterKit,
  TiptapTable.configure({ resizable: true }),
  TableRow,
  TableHeader,
  TableCell,
  TaskList,
  TaskItem.configure({ nested: true }),
  PromptXmlDecorations,
  Markdown,
];

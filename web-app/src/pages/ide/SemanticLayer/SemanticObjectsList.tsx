import { useMemo } from "react";
import {
  SidebarMenu,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem
} from "@/components/ui/shadcn/sidebar";
import useTopicFiles from "@/hooks/api/useTopicFiles";
import useViewFiles from "@/hooks/api/useViewFiles";
import { detectFileType } from "@/utils/fileTypes";
import { getFileTypeIcon } from "../Files/FilesSidebar/utils";

type SemanticObjectKind = "topic" | "view";

export interface SemanticObjectItem {
  kind: SemanticObjectKind;
  label: string;
  path: string;
}

interface SemanticObjectsListProps {
  selectedPath: string | null;
  onSelect: (item: SemanticObjectItem) => void;
}

/**
 * Flat list of `.topic.yml` + `.view.yml` files. Same item styling as the
 * IDE's Files → Objects → "Semantic Model" group, but without the redundant
 * outer collapsible header (the parent tab is already titled "Semantic Model").
 */
export default function SemanticObjectsList({ selectedPath, onSelect }: SemanticObjectsListProps) {
  const { topicFiles } = useTopicFiles();
  const { viewFiles } = useViewFiles();

  const items = useMemo<SemanticObjectItem[]>(() => {
    const merged: SemanticObjectItem[] = [
      ...topicFiles.map((t) => ({ kind: "topic" as const, label: t.label, path: t.path })),
      ...viewFiles.map((v) => ({ kind: "view" as const, label: v.label, path: v.path }))
    ];
    return merged.sort((a, b) => a.label.localeCompare(b.label));
  }, [topicFiles, viewFiles]);

  return (
    <SidebarMenu className='py-2' data-testid='semantic-objects-list'>
      <SidebarMenuSub className='border-l-0'>
        {items.map((item) => {
          const fileType = detectFileType(item.path);
          const name = item.path.split("/").pop() ?? item.path;
          const Icon = getFileTypeIcon(fileType, name);
          const isActive = selectedPath === item.path;
          return (
            <SidebarMenuSubItem key={item.path}>
              <SidebarMenuSubButton
                onClick={() => onSelect(item)}
                isActive={isActive}
                className='text-muted-foreground transition-colors duration-150 ease-in hover:text-sidebar-foreground'
                data-testid={`semantic-objects-item-${item.kind}-${item.label}`}
              >
                {Icon && <Icon />}
                <span className='flex-1 truncate'>{item.label}</span>
              </SidebarMenuSubButton>
            </SidebarMenuSubItem>
          );
        })}
        {items.length === 0 && (
          <li className='px-2 py-1 text-muted-foreground text-xs'>No topics or views yet.</li>
        )}
      </SidebarMenuSub>
    </SidebarMenu>
  );
}

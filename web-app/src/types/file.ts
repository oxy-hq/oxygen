export interface FileTreeModel {
  name: string;
  path: string;
  is_dir: boolean;
  children: FileTreeModel[];
}

interface RepoSection {
  name: string;
  sync_status: "ready" | "cloning";
  git_url?: string;
}

export interface FileTreeResponse {
  primary: FileTreeModel[];
  repositories: RepoSection[];
}

export interface FileStatus {
  path: string;
  status: "M" | "A" | "D" | "U";
  insert: number;
  delete: number;
}

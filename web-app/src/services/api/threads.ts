import type { Answer, ThreadCreateRequest, ThreadItem, ThreadsResponse } from "@/types/chat";
import { apiBaseURL } from "../env";
import { apiClient } from "./axios";
import fetchSSE from "./fetchSSE";

export class ThreadService {
  static async listThreads(
    projectId: string,
    page?: number,
    limit?: number,
    search?: string
  ): Promise<ThreadsResponse> {
    const params = new URLSearchParams();
    if (page !== undefined) params.append("page", page.toString());
    if (limit !== undefined) params.append("limit", limit.toString());
    if (search) params.append("search", search);

    let url = `/${projectId}/threads`;
    const paramsStr = params.toString();
    if (paramsStr) {
      url += `?${paramsStr}`;
    }
    const response = await apiClient.get(url);
    return response.data;
  }

  static async createThread(projectId: string, request: ThreadCreateRequest): Promise<ThreadItem> {
    const response = await apiClient.post(`/${projectId}/threads`, request);
    return response.data;
  }

  static async getThread(projectId: string, threadId: string): Promise<ThreadItem> {
    const response = await apiClient.get(`/${projectId}/threads/${threadId}`);
    return response.data;
  }

  static async deleteThread(projectId: string, threadId: string): Promise<void> {
    const response = await apiClient.delete(`/${projectId}/threads/${threadId}`);
    return response.data;
  }

  static async bulkDeleteThreads(projectId: string, threadIds: string[]): Promise<void> {
    const response = await apiClient.post(`/${projectId}/threads/bulk-delete`, {
      thread_ids: threadIds
    });
    return response.data;
  }

  static async deleteAllThreads(projectId: string): Promise<void> {
    const response = await apiClient.delete(`/${projectId}/threads`);
    return response.data;
  }

  static async askTask(
    projectId: string,
    taskId: string,
    question: string | null,
    onReadStream: (answer: Answer) => void,
    onMessageSent?: () => void
  ): Promise<void> {
    const url = `${apiBaseURL}/${projectId}/threads/${taskId}/task`;
    await fetchSSE(url, {
      body: { question },
      onMessage: onReadStream,
      onOpen: onMessageSent,
      eventTypes: ["message", "error"]
    });
  }

  static async askAgent(
    projectId: string,
    threadId: string,
    question: string | null,
    onReadStream: (answer: Answer) => void,
    onMessageSent?: () => void
  ): Promise<void> {
    const url = `${apiBaseURL}/${projectId}/threads/${threadId}/agent`;
    await fetchSSE(url, {
      body: { question },
      onMessage: onReadStream,
      onOpen: onMessageSent,
      eventTypes: ["message", "error"]
    });
  }

  static async stopThread(projectId: string, threadId: string): Promise<void> {
    const url = `${apiBaseURL}/${projectId}/threads/${threadId}/stop`;
    await apiClient.post(url);
  }
}

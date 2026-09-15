import type { Message } from "@/types/chat";

const STREAMING_MESSAGE_PREFIX = "temp-";

const DEFAULT_USAGE = {
  inputTokens: 0,
  outputTokens: 0
} as const;

export class MessageFactory {
  static createStreamingMessage(threadId: string, prefix = "streaming"): Message {
    return {
      id: `${STREAMING_MESSAGE_PREFIX}${prefix}-${Date.now()}`,
      content: "",
      references: [],
      steps: [],
      is_human: false,
      isStreaming: true,
      usage: DEFAULT_USAGE,
      artifacts: {},
      thread_id: threadId,
      created_at: new Date().toISOString(),
      file_path: ""
    };
  }

  static createUserMessage(content: string, threadId: string): Message {
    return {
      id: `${STREAMING_MESSAGE_PREFIX}user-${Date.now()}`,
      content,
      usage: DEFAULT_USAGE,
      references: [],
      steps: [],
      is_human: true,
      isStreaming: false,
      artifacts: {},
      thread_id: threadId,
      created_at: new Date().toISOString(),
      file_path: ""
    };
  }

  static createErrorMessage(
    threadId: string,
    errorContent: string,
    existingMessageId?: string
  ): Message {
    return {
      id: existingMessageId ?? `${STREAMING_MESSAGE_PREFIX}error-${Date.now()}`,
      content: errorContent,
      references: [],
      steps: [],
      is_human: false,
      isStreaming: false,
      usage: DEFAULT_USAGE,
      artifacts: {},
      thread_id: threadId,
      created_at: new Date().toISOString(),
      file_path: ""
    };
  }

  static completeStreamingMessage(message: Message): Message {
    return {
      ...message,
      isStreaming: false
    };
  }
}

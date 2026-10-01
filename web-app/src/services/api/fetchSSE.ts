import { fetchEventSource } from "@microsoft/fetch-event-source";
import { toast } from "sonner";
import {
  PREVIEW_HEADER,
  previewRequestHeaders,
  readPreviewReadOnlyBody,
  reportPreviewServed
} from "@/libs/utils/preview";

const fetchSSE = async <T>(
  url: string,
  options: {
    method?: string;
    body?: unknown;
    onMessage: (data: T) => void;
    onOpen?: () => void;
    onClose?: () => void;
    onError?: (error: Error) => void;
    eventTypes?: string[];
    signal?: AbortSignal | null;
  }
) => {
  const {
    method = "POST",
    body,
    onMessage,
    onOpen,
    onClose,
    onError,
    signal,
    eventTypes = ["message"]
  } = options;
  const token = localStorage.getItem("auth_token");
  await fetchEventSource(url, {
    method,
    headers: {
      "Content-Type": "application/json",
      Authorization: token ?? "",
      ...previewRequestHeaders()
    },
    openWhenHidden: true,
    body: body ? JSON.stringify(body) : undefined,
    signal,
    async onopen(res) {
      reportPreviewServed(res.headers.get(PREVIEW_HEADER));
      if (res.status === 409) {
        // A run started while pinned to a preview. The server's refusal is
        // written for the person, so it is the error — not "status: 409".
        const body = await res.json().catch(() => null);
        const readOnly = readPreviewReadOnlyBody(res.status, body);
        if (readOnly) {
          toast.error(readOnly, { id: "preview-read-only" });
          throw new Error(readOnly);
        }
      }
      if (res.status !== 200) {
        throw new Error(`SSE connection failed with status: ${res.status}`);
      }
      onOpen?.();
    },
    onmessage(ev) {
      if (!ev.event || eventTypes.includes(ev.event)) {
        try {
          const data = JSON.parse(ev.data);
          onMessage(data);
        } catch (error) {
          console.error("Error parsing SSE data:", error);
        }
      }
    },
    onerror(err) {
      console.error("SSE error:", err);
      onError?.(err);
      throw err;
    },
    onclose() {
      onClose?.();
    }
  });
};

export default fetchSSE;

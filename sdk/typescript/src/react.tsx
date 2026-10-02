import { createContext, type ReactNode, useContext, useEffect, useState } from "react";
import type { OxyConfig } from "./config";
import { OxySDK } from "./sdk";

/**
 * Context value provided to child components
 */
export interface OxyContextValue {
  sdk: OxySDK | null;
  isLoading: boolean;
  error: Error | null;
}

/**
 * React context for OxySDK
 */
const OxyContext = createContext<OxyContextValue | undefined>(undefined);

/**
 * Props for OxyProvider component
 */
export interface OxyProviderProps {
  children: ReactNode;
  config?: Partial<OxyConfig>;
  /**
   * If true, uses async initialization (supports postMessage auth in iframes)
   * If false, uses synchronous initialization with provided config
   */
  useAsync?: boolean;
  /**
   * Optional app path to load initial app data from upon initialization
   */
  appPath?: string;
  /**
   * Optional initial files to preload into the SDK as a mapping of filename to content
   */
  files?: Record<string, string>;
  /**
   * Called when SDK is successfully initialized
   */
  onReady?: (sdk: OxySDK) => void;
  /**
   * Called when initialization fails
   */
  onError?: (error: Error) => void;
}

/**
 * Provider component that initializes and provides OxySDK to child components
 *
 * @example
 * ```tsx
 * // Synchronous initialization with config
 * function App() {
 *   return (
 *     <OxyProvider config={{
 *       apiKey: 'your-key',
 *       projectId: 'your-project',
 *       baseUrl: 'https://api.oxy.tech'
 *     }}>
 *       <Dashboard />
 *     </OxyProvider>
 *   );
 * }
 * ```
 *
 * @example
 * ```tsx
 * // Async initialization (for iframe/postMessage auth)
 * function App() {
 *   return (
 *     <OxyProvider
 *       useAsync
 *       config={{ parentOrigin: 'https://app.example.com' }}
 *     >
 *       <Dashboard />
 *     </OxyProvider>
 *   );
 * }
 * ```
 *
 * @example
 * ```tsx
 * // With environment variables
 * import { createConfig } from '@oxy/sdk';
 *
 * function App() {
 *   return (
 *     <OxyProvider config={createConfig()}>
 *       <Dashboard />
 *     </OxyProvider>
 *   );
 * }
 * ```
 */
export function OxyProvider({
  children,
  config,
  useAsync = false,
  appPath,
  files,
  onReady,
  onError
}: OxyProviderProps) {
  const [sdk, setSdk] = useState<OxySDK | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    let mounted = true;
    let sdkInstance: OxySDK | null = null;

    async function initializeSDK() {
      try {
        setIsLoading(true);
        setError(null);

        if (useAsync) {
          // Async initialization (supports postMessage auth)
          sdkInstance = await OxySDK.create(config);
          if (appPath) {
            await sdkInstance.loadAppData(appPath);
          }

          if (files) {
            const fileEntries = Object.entries(files).map(([tableName, filePath]) => ({
              tableName,
              filePath
            }));
            await sdkInstance.loadFiles(fileEntries);
          }
        } else {
          // Sync initialization with provided config
          if (!config) {
            throw new Error(
              "Config is required when useAsync is false. Either provide config or set useAsync=true."
            );
          }
          sdkInstance = new OxySDK(config as OxyConfig);
        }

        if (mounted) {
          setSdk(sdkInstance);
          setIsLoading(false);
          onReady?.(sdkInstance);
        }
      } catch (err) {
        const error = err instanceof Error ? err : new Error("Failed to initialize SDK");

        if (mounted) {
          setError(error);
          setIsLoading(false);
          onError?.(error);
        }
      }
    }

    // Never rejects: its whole body is one try/catch that reports through
    // `setError` / `onError`.
    void initializeSDK();

    // Cleanup function
    return () => {
      mounted = false;
      if (sdkInstance) {
        sdkInstance.close().catch(console.error);
      }
    };
  }, [config, useAsync, onReady, onError, appPath, files]);

  return <OxyContext.Provider value={{ sdk, isLoading, error }}>{children}</OxyContext.Provider>;
}

/**
 * Hook to access OxySDK from child components
 *
 * @throws {Error} If used outside of OxyProvider
 * @returns {OxyContextValue} The SDK instance, loading state, and error
 *
 * @example
 * ```tsx
 * function Dashboard() {
 *   const { sdk, isLoading, error } = useOxy();
 *
 *   useEffect(() => {
 *     if (sdk) {
 *       sdk.loadAppData('dashboard.app.yml')
 *         .then(() => sdk.query('SELECT * FROM my_table'))
 *         .then(result => console.log(result));
 *     }
 *   }, [sdk]);
 *
 *   if (isLoading) return <div>Loading SDK...</div>;
 *   if (error) return <div>Error: {error.message}</div>;
 *   if (!sdk) return null;
 *
 *   return <div>Dashboard</div>;
 * }
 * ```
 */
export function useOxy(): OxyContextValue {
  const context = useContext(OxyContext);

  if (context === undefined) {
    throw new Error("useOxy must be used within an OxyProvider");
  }

  return context;
}

/**
 * Hook to access OxySDK that throws if not ready
 *
 * This is a convenience hook that returns the SDK directly or throws an error if not initialized.
 * Use this when you know the SDK should be ready.
 *
 * @throws {Error} If used outside of OxyProvider or if SDK is not initialized
 * @returns {OxySDK} The SDK instance
 *
 * @example
 * ```tsx
 * function DataTable() {
 *   const sdk = useOxySDK();
 *   const [data, setData] = useState(null);
 *
 *   useEffect(() => {
 *     sdk.loadFile('data.parquet', 'data')
 *       .then(() => sdk.query('SELECT * FROM data LIMIT 100'))
 *       .then(setData);
 *   }, [sdk]);
 *
 *   return <table>...</table>;
 * }
 * ```
 */
export function useOxySDK(): OxySDK {
  const { sdk, isLoading, error } = useOxy();

  if (error) {
    throw error;
  }

  if (isLoading || !sdk) {
    throw new Error("OxySDK is not yet initialized");
  }

  return sdk;
}

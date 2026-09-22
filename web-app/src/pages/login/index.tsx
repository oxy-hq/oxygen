import { Spinner } from "@/components/ui/shadcn/spinner";
import { useKioskDevice } from "@/hooks/auth/useFrontline";
import useTheme from "@/stores/useTheme";
import LoginForm, { KioskLogin } from "./LoginForm";

export default function LoginPage() {
  const { theme } = useTheme();
  const { data: device, isPending: isProbingKiosk } = useKioskDevice();

  // On an enrolled kiosk the page is the crew's name board, and it takes the
  // whole screen: a centred column left most of a tablet empty while the
  // names scrolled in a box.
  if (device?.bound) {
    return <KioskLogin device={device} />;
  }

  return (
    <div className='grid h-full w-full overflow-auto'>
      <div className='flex flex-col gap-4 p-6 md:p-10'>
        <div className='flex justify-center gap-2 md:justify-start'>
          <a href='#' className='flex items-center gap-2 font-medium'>
            <img src={theme === "dark" ? "/oxygen-dark.svg" : "/oxygen-light.svg"} alt='Oxygen' />
            <span className='truncate font-medium text-sm'>Oxygen</span>
          </a>
        </div>
        <div className='flex flex-1 items-center justify-center'>
          <div className='w-full max-w-xs'>
            {/* Hold the page until the probe answers. Rendering the account
                options first would flash exactly what a kiosk hides. The probe
                never throws, and for a browser without a kiosk cookie the
                server runs no query before answering. */}
            {isProbingKiosk ? (
              <div className='flex justify-center py-10' data-testid='login-probing'>
                <Spinner />
              </div>
            ) : (
              <LoginForm />
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

// The API has no "is registration open?" endpoint; the only signal is a 403 from POST /auth/register.
// Remember it for the tab so the login page can stop offering the link.
const KEY = "cyanotype.registration-closed";

export const registrationKnownClosed = (): boolean => sessionStorage.getItem(KEY) === "1";
export const markRegistrationClosed = (): void => sessionStorage.setItem(KEY, "1");

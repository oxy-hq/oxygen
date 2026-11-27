/** Which attempt an opened check belongs to: what its evidence needs to fetch a
 *  picture or mark the changes seen. The attempt's number is on the check itself. */
export interface ChecksScope {
  appId: string;
  /** The build's row id, as the server answered it. */
  build: string;
  environment: string;
  /** The app holds Promote until a person has seen the changed screens. */
  seenRequired: boolean;
}

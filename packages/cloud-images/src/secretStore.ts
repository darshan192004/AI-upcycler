export interface SecretStore {
  get(scope: 'provider:openai'): Promise<string | undefined>;
  set(scope: 'provider:openai', value: string): Promise<void>;
  delete(scope: 'provider:openai'): Promise<void>;
}

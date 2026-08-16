// HTTP client for a sim's `/_sim/*` control API (e2e-sims/src/control.rs).

export type SimRule = {
  matcher?: {
    method?: string;
    pathGlob?: string;
    bodyContains?: string;
    nthCall?: number;
  };
  action: {
    type:
      | 'status'
      | 'delay'
      | 'stall'
      | 'drop'
      | 'truncate'
      | 'corrupt'
      | 'passthrough';
    code?: number;
    body?: unknown;
    ms?: number;
    afterBytes?: number;
    fraction?: number;
    mode?: string;
  };
  times?: number;
};

export type SimRequest = {
  method: string;
  path: string;
  query?: string | null;
  body: string;
  tsMs: number;
};

export class SimClient {
  /** Mirror of what we pushed; the sim has no rules GET. */
  private active: SimRule[] = [];

  constructor(readonly baseUrl: string) {}

  rules(): SimRule[] {
    return this.active;
  }

  private async call(
    method: string,
    path: string,
    body?: unknown,
  ): Promise<Response> {
    const res = await fetch(`${this.baseUrl}${path}`, {
      method,
      ...(body === undefined
        ? {}
        : {
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify(body),
          }),
    });
    if (!res.ok) {
      throw new Error(
        `sim ${method} ${path} -> ${res.status}: ${await res.text()}`,
      );
    }
    return res;
  }

  async addRule(rule: SimRule | SimRule[]): Promise<void> {
    await this.call('POST', '/_sim/rules', rule);
    this.active.push(...(Array.isArray(rule) ? rule : [rule]));
  }

  async clearRules(): Promise<void> {
    await this.call('DELETE', '/_sim/rules');
    this.active = [];
  }

  async reset(): Promise<void> {
    await this.call('POST', '/_sim/reset');
    this.active = [];
  }

  async seed(data: unknown): Promise<void> {
    await this.call('POST', '/_sim/seed', data);
  }

  async requests(): Promise<SimRequest[]> {
    return (await this.call('GET', '/_sim/requests')).json();
  }
}

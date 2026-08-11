## Description: <br>
Cheap first-pass web discovery without launching Chrome: fetch SSR pages, run bounded JavaScript, find routes/forms/API endpoints, extract structured data, and detect bot-wall or browser-only escalation points. <br>

This skill is ready for commercial/non-commercial use. <br>

## Publisher: <br>
[protostatis](https://clawhub.ai/user/protostatis) <br>

### License/Terms of Use: <br>
MIT-0 <br>


## Use Case: <br>
Developers and agents use this skill for low-cost first-pass web discovery, structured extraction, route/form/API discovery, and deciding when a full managed browser is needed. It is intended for public pages and explicitly authorized authenticated browsing with host-scoped cookies. <br>

### Deployment Geography for Use: <br>
Global <br>

## Known Risks and Mitigations: <br>
Risk: Session cookies can authenticate as the user and may expose account access if reused broadly. <br>
Mitigation: Use cookies only for the exact site authorized by the user, clear cookies after the task, and close the session before unrelated browsing. <br>
Risk: Authenticated actions such as posting, purchasing, deleting, sending, transferring, or changing settings can modify user accounts. <br>
Mitigation: Require explicit user confirmation before any authenticated state-changing action. <br>
Risk: A challenge-cookie solver can return browser cookies if exposed beyond the local machine. <br>
Mitigation: Keep solver services bound to loopback, use host allowlists for private or internal targets, and do not expose unauthenticated solver endpoints publicly. <br>
Risk: Page JavaScript and DOM-derived strings are untrusted. <br>
Mitigation: Run only agent-authored diagnostic or extraction JavaScript and never eval content copied from a page. <br>


## Reference(s): <br>
- [ClawHub skill page](https://clawhub.ai/protostatis/skills/unbrowser) <br>
- [Project homepage](https://github.com/protostatis/unbrowser) <br>
- [RPC methods](https://github.com/protostatis/unbrowser#rpc-methods) <br>


## Skill Output: <br>
**Output Type(s):** [text, markdown, code, shell commands, configuration, guidance] <br>
**Output Format:** [Markdown guidance with shell, JSON-RPC, and Python examples] <br>
**Output Parameters:** [1D] <br>
**Other Properties Related to Output:** [May propose unbrowser CLI commands, JSON-RPC requests, Python snippets, extraction strategy, escalation decisions, and safety guidance.] <br>

## Skill Version(s): <br>
0.0.19 (source: server release metadata and SKILL.md frontmatter) <br>

## Ethical Considerations: <br>
Users should evaluate whether this skill is appropriate for their environment, review any generated or modified files before relying on them, and apply their organization's safety, security, and compliance requirements before deployment. <br>

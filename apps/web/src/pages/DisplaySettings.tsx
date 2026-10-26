import { Block, Group, Loading, Row, Section } from "../settings/kit";
import { Segmented, Switch } from "../ui/Controls";
import { usePrefs } from "../usePrefs";

/** Settings > Display: the per-user switches that shape how the library looks (GET/PUT /assets/prefs). */
export function DisplaySettings() {
  const { prefs, update } = usePrefs();
  if (!prefs) return <Loading />;
  return (
    <div>
      <Section title="Photos" description="How the photo grid lists your library.">
        <Group>
          <Block>
            <Switch
              label="Stack similar photos"
              hint="Photos taken together show as one tile in the timeline. Turn off to list every photo."
              checked={prefs.stacks_collapsed}
              onChange={(on) => void update({ stacks_collapsed: on })}
            />
          </Block>
          <Row label="Order" detail="Newest first, by when the photo was taken or when it was added.">
            <Segmented
              label="Order"
              value={prefs.library_sort}
              onChange={(v) => void update({ library_sort: v })}
              options={[
                { value: "taken", label: "Taken" },
                { value: "added", label: "Added" },
              ]}
            />
          </Row>
          {/* The grid's style (square or justified, `grid_style`) is a Row of the same kind: a Segmented in this group. */}
        </Group>
      </Section>

      <Section title="Memories" description="Photos from this day in earlier years.">
        <Group>
          <Block>
            <Switch
              label="Show memories"
              hint="The On this day strip on Collections and the Memories page."
              checked={prefs.memories_enabled}
              onChange={(on) => void update({ memories_enabled: on })}
            />
          </Block>
        </Group>
      </Section>
    </div>
  );
}

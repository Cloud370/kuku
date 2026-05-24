import type { Meta, StoryObj } from "@storybook/react";
import { Card } from "./Card";

const meta: Meta<typeof Card> = {
  title: "UI/Card",
  component: Card,
};

export default meta;
type Story = StoryObj<typeof Card>;

export const Default: Story = {
  render: () => (
    <Card>
      <Card.Header>Card Header</Card.Header>
      <Card.Body>Card body content goes here.</Card.Body>
      <Card.Footer>Card Footer</Card.Footer>
    </Card>
  ),
};

export const BodyOnly: Story = {
  render: () => (
    <Card>
      <Card.Body>A simple card with just a body.</Card.Body>
    </Card>
  ),
};

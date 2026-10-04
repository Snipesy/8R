package com.example.slots;

import java.util.HashMap;
import java.util.Map;

interface Source {
    State state();

    boolean offer(Object value);
}

final class State {
    final String label;

    State(String label) {
        this.label = label;
    }
}

abstract class Base {
    private final State current = new State("base-" + System.nanoTime() % 1);

    private int offered;

    // Implement Source's methods for subclasses that implement Source; Base itself doesn't.
    public State state() {
        return current;
    }

    public boolean offer(Object value) {
        offered++;
        return value != null;
    }

    int offered() {
        return offered;
    }
}

final class Flow extends Base implements Source {
}

// A second subclass keeps Base from being merged into Flow.
final class Plain extends Base {
    private final String tag = "plain" + System.nanoTime() % 1;

    @Override
    public State state() {
        return new State(tag);
    }
}

final class Other implements Source {
    @Override
    public State state() {
        return new State("other");
    }

    @Override
    public boolean offer(Object value) {
        return false;
    }
}

final class HomeModel {
    final String name = "home";
}

final class DetailModel {
    final String name = "detail";
}

final class Registry {
    private final Map<String, Object> byName = new HashMap<>();

    Registry() {
        // Rewritten by -adaptclassstrings to the obfuscated names.
        byName.put("com.example.slots.HomeModel", new HomeModel());
        byName.put("com.example.slots.DetailModel", new DetailModel());
    }

    Object get(Class<?> c) {
        return byName.get(c.getName());
    }
}

public class Main {
    static String describe(Source s) {
        return s.state().label + ":" + s.offer("x");
    }

    public static void main(String[] args) {
        Flow f = new Flow();
        Source[] all = {f, new Other()};
        for (Source s : all) {
            System.out.println(describe(s));
        }
        System.out.println("offered " + f.offered());
        Base[] bases = {f, new Plain()};
        for (Base b : bases) {
            System.out.println(b.state().label);
        }
        Registry r = new Registry();
        HomeModel h = (HomeModel) r.get(HomeModel.class);
        DetailModel d = (DetailModel) r.get(DetailModel.class);
        System.out.println((h == null ? "missing" : h.name) + " " + (d == null ? "missing" : d.name));
    }
}

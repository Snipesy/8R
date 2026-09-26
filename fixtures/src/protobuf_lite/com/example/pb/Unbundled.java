package com.example.pb;

import com.google.protobuf.GeneratedMessageLite;

/** The shape protoc's lite codegen gives message Unbundled. */
public final class Unbundled extends GeneratedMessageLite {
    public static final int X_FIELD_NUMBER = 1;
    public static final int Y_FIELD_NUMBER = 2;
    public static final int Z_FIELD_NUMBER = 7;
    private int x_;
    private int y_;
    private int z_;
    public Unbundled(int x, int y, int z) {
        this.x_ = x;
        this.y_ = y;
        this.z_ = z;
    }
    @Override protected String describe() { return "U:" + x_ + "," + y_ + "," + z_; }
}
